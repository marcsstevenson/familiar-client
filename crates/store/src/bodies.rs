// SPDX-License-Identifier: MIT OR Apache-2.0
//! Content-addressed body storage.

use crate::{Error, Result};
use rusqlite::{Connection, OptionalExtension, params};
use std::fmt;
use std::path::{Path, PathBuf};

/// Bodies that compress to this many bytes or fewer are stored inline in
/// SQLite rather than in their own file. One 4 KB block is the smallest a
/// file can take on disk.
pub const INLINE_BODY_MAX: usize = 4096;

const ZSTD_LEVEL: i32 = 3;

/// The BLAKE3 hash of a body's raw bytes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BodyHash(pub [u8; 32]);

impl BodyHash {
    /// Hashes raw body bytes.
    pub fn of(bytes: &[u8]) -> Self {
        BodyHash(*blake3::hash(bytes).as_bytes())
    }

    /// Lower-case hex.
    pub fn to_hex(&self) -> String {
        blake3::Hash::from_bytes(self.0).to_hex().to_string()
    }

    pub(crate) fn from_blob(v: Vec<u8>) -> rusqlite::Result<Self> {
        let arr: [u8; 32] = v.try_into().map_err(|v: Vec<u8>| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Blob,
                format!("body hash is {} bytes, not 32", v.len()).into(),
            )
        })?;
        Ok(BodyHash(arr))
    }
}

impl fmt::Debug for BodyHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BodyHash({})", self.to_hex())
    }
}

impl fmt::Display for BodyHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl serde::Serialize for BodyHash {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

/// `bodies/ab/cd/<hex>.zst`
pub(crate) fn body_path(root: &Path, hash: &BodyHash) -> PathBuf {
    let hex = hash.to_hex();
    root.join(&hex[0..2])
        .join(&hex[2..4])
        .join(format!("{hex}.zst"))
}

/// Parses a body file name back to its hash.
pub(crate) fn hash_of_file_name(name: &str) -> Option<BodyHash> {
    let hex = name.strip_suffix(".zst")?;
    blake3::Hash::from_hex(hex)
        .ok()
        .map(|h| BodyHash(*h.as_bytes()))
}

/// Stores a body if it isn't stored yet. Call inside the writer's
/// transaction. A file is written (atomically, via a temporary name) before
/// its row; if the transaction then rolls back, the file is an orphan that
/// [`crate::Writer::gc_bodies`] removes.
pub(crate) fn put(conn: &Connection, root: &Path, bytes: &[u8]) -> Result<BodyHash> {
    let hash = BodyHash::of(bytes);
    let exists = conn
        .query_row(
            "SELECT 1 FROM bodies WHERE hash = ?1",
            [&hash.0[..]],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if exists {
        return Ok(hash);
    }
    let packed =
        zstd::bulk::compress(bytes, ZSTD_LEVEL).map_err(|e| Error::io("compress body", root, e))?;
    let inline = packed.len() <= INLINE_BODY_MAX;
    if !inline {
        write_file(root, &hash, &packed)?;
    }
    conn.execute(
        "INSERT INTO bodies (hash, size, stored_size, data) VALUES (?1, ?2, ?3, ?4)",
        params![
            &hash.0[..],
            bytes.len() as i64,
            packed.len() as i64,
            inline.then_some(&packed[..])
        ],
    )?;
    Ok(hash)
}

fn write_file(root: &Path, hash: &BodyHash, packed: &[u8]) -> Result<()> {
    let path = body_path(root, hash);
    if path.exists() {
        // Left by a rolled-back transaction; same hash, so same content.
        return Ok(());
    }
    let dir = path.parent().expect("body path has a parent");
    std::fs::create_dir_all(dir).map_err(|e| Error::io("create", dir, e))?;
    let tmp = path.with_extension("zst.tmp");
    std::fs::write(&tmp, packed).map_err(|e| Error::io("write", &tmp, e))?;
    std::fs::rename(&tmp, &path).map_err(|e| Error::io("rename", &path, e))?;
    Ok(())
}

/// Reads and decompresses a body, checking it against its hash.
pub(crate) fn get(conn: &Connection, root: &Path, hash: &BodyHash) -> Result<Vec<u8>> {
    let row: Option<(i64, Option<Vec<u8>>)> = conn
        .query_row(
            "SELECT size, data FROM bodies WHERE hash = ?1",
            [&hash.0[..]],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((size, data)) = row else {
        return Err(Error::NotFound(format!("body {hash}")));
    };
    let packed = match data {
        Some(d) => d,
        None => {
            let path = body_path(root, hash);
            std::fs::read(&path).map_err(|e| Error::io("read", &path, e))?
        }
    };
    let corrupt = |reason: String| Error::CorruptBody {
        hash: hash.to_hex(),
        reason,
    };
    let raw = zstd::bulk::decompress(&packed, size.max(0) as usize)
        .map_err(|e| corrupt(e.to_string()))?;
    if BodyHash::of(&raw) != *hash {
        return Err(corrupt("content does not match hash".into()));
    }
    Ok(raw)
}
