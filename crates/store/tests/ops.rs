// SPDX-License-Identifier: MIT OR Apache-2.0
//! The durable op queue.

mod common;

use common::*;
use familiar_store::*;

fn op(account: AccountId, kind: &str, target: &str) -> NewOp {
    NewOp {
        account,
        kind: kind.into(),
        target: target.into(),
        payload: r#"{"x":1}"#.into(),
        ..Default::default()
    }
}

#[test]
fn ops_on_one_target_run_in_order_and_others_run_alongside() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = Writer::open(dir.path()).unwrap();
    let a = account(&mut w, "hotmail");
    let (read, mv, other) = w
        .write(|tx| {
            Ok((
                tx.enqueue_op(&op(a, "mark_read", "m1"))?,
                tx.enqueue_op(&op(a, "move", "m1"))?,
                tx.enqueue_op(&op(a, "mark_read", "m2"))?,
            ))
        })
        .unwrap();

    w.write(|tx| {
        let first = tx.claim_op(100)?.unwrap();
        assert_eq!(
            (first.id, first.state, first.attempts),
            (read, OpState::InFlight, 1)
        );
        // "move m1" waits for "mark_read m1"; "mark_read m2" doesn't.
        assert_eq!(tx.claim_op(100)?.unwrap().id, other);
        assert!(tx.claim_op(100)?.is_none());
        tx.complete_op(read)?;
        assert_eq!(tx.claim_op(100)?.unwrap().id, mv);
        Ok(())
    })
    .unwrap();
}

#[test]
fn retry_waits_for_not_before_and_blocks_later_ops_on_the_target() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = Writer::open(dir.path()).unwrap();
    let a = account(&mut w, "hotmail");
    w.write(|tx| {
        let first = tx.enqueue_op(&op(a, "mark_read", "m1"))?;
        let second = tx.enqueue_op(&op(a, "move", "m1"))?;
        assert_eq!(tx.claim_op(100)?.unwrap().id, first);
        tx.retry_op(first, "429, Retry-After 30", 130)?;
        assert!(
            tx.claim_op(110)?.is_none(),
            "backing off, and second must wait"
        );
        let again = tx.claim_op(130)?.unwrap();
        assert_eq!((again.id, again.attempts), (first, 2));
        assert_eq!(again.last_error.as_deref(), Some("429, Retry-After 30"));
        // A failed op no longer holds up the target.
        tx.fail_op(first, "403")?;
        assert_eq!(tx.claim_op(130)?.unwrap().id, second);
        Ok(())
    })
    .unwrap();
}

#[test]
fn in_flight_ops_return_to_the_queue_when_the_writer_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let a;
    let id;
    {
        let mut w = Writer::open(dir.path()).unwrap();
        a = account(&mut w, "hotmail");
        id = w
            .write(|tx| {
                let id = tx.enqueue_op(&op(a, "send", "draft-1"))?;
                tx.claim_op(i64::MAX)?;
                Ok(id)
            })
            .unwrap();
        // The daemon dies here, mid-send.
    }
    let mut w = Writer::open(dir.path()).unwrap();
    let o = w.write(|tx| tx.op(id)).unwrap().unwrap();
    assert_eq!(o.state, OpState::Pending);
    let claimed = w.write(|tx| tx.claim_op(i64::MAX)).unwrap().unwrap();
    assert_eq!((claimed.id, claimed.attempts), (id, 2));
    assert_eq!(
        claimed.idempotency_key, o.idempotency_key,
        "same key on retry"
    );
}

#[test]
fn op_and_local_change_commit_or_roll_back_together() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = Writer::open(dir.path()).unwrap();
    let a = account(&mut w, "hotmail");
    let inbox = folder(&mut w, a, "Inbox");
    let m = w
        .write(|tx| tx.upsert_message(&msg(a, inbox, "m1", "c", 1, false)))
        .unwrap();
    let out: Result<()> = w.write(|tx| {
        tx.set_read(m, true)?;
        tx.enqueue_op(&op(a, "mark_read", "m1"))?;
        Err(Error::NotFound("simulated failure".into()))
    });
    assert!(out.is_err());
    let r = Reader::open(dir.path()).unwrap();
    assert!(!r.message(m).unwrap().unwrap().is_read);
    assert!(r.open_ops().unwrap().is_empty());
    assert_eq!(r.folder(inbox).unwrap().unwrap().unread, 1);
}

#[test]
fn idempotency_keys_are_unique_and_done_ops_can_be_pruned() {
    let dir = tempfile::tempdir().unwrap();
    let mut w = Writer::open(dir.path()).unwrap();
    let a = account(&mut w, "hotmail");
    let keys: Vec<String> = w
        .write(|tx| {
            let mut keys = Vec::new();
            for i in 0..100 {
                let id = tx.enqueue_op(&op(a, "mark_read", &format!("m{i}")))?;
                keys.push(tx.op(id)?.unwrap().idempotency_key);
            }
            Ok(keys)
        })
        .unwrap();
    let mut dedup = keys.clone();
    dedup.sort();
    dedup.dedup();
    assert_eq!(dedup.len(), 100);

    let mut given = op(a, "mark_read", "x");
    given.idempotency_key = Some(keys[0].clone());
    assert!(
        w.write(|tx| tx.enqueue_op(&given)).is_err(),
        "duplicate key refused"
    );

    w.write(|tx| {
        while let Some(o) = tx.claim_op(i64::MAX)? {
            tx.complete_op(o.id)?;
        }
        Ok(())
    })
    .unwrap();
    assert!(
        Reader::open(dir.path())
            .unwrap()
            .open_ops()
            .unwrap()
            .is_empty()
    );
    assert_eq!(w.write(|tx| tx.prune_done_ops(i64::MAX)).unwrap(), 100);
}
