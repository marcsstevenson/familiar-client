// SPDX-License-Identifier: MIT OR Apache-2.0
//! Outlook contact writes against the mock Graph mailbox.

mod common;

use familiar_mock_graph::{Contact, EmailAddress, MockGraph};
use familiar_provider_graph::{ContactsApi, Error, Priority};
use familiar_store::{AccountId, ContactChanges};

#[tokio::test]
async fn create_update_clear_and_delete() {
    let mock = MockGraph::start().await.unwrap();
    let api = ContactsApi::new(common::client(&mock, "test-token"));
    let account = AccountId(1);
    let created = api
        .save_contact(
            account,
            None,
            &ContactChanges {
                name: Some("Chris".into()),
                company: Some("Example".into()),
                emails: Some(vec!["chris@example.com".into()]),
                phones: Some(vec!["555-0100".into()]),
            },
        )
        .await
        .unwrap();
    assert!(!created.remote_id.is_empty());
    let id = created.remote_id.as_str();
    let renamed = api
        .save_contact(
            account,
            Some(id),
            &ContactChanges {
                name: Some("Chris Smith".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(renamed.display_name, "Chris Smith");
    assert_eq!(renamed.company, "Example");
    assert_eq!(renamed.emails[0].addr, "chris@example.com");
    assert_eq!(renamed.phones, ["555-0100"]);

    let cleared = api
        .save_contact(
            account,
            Some(id),
            &ContactChanges {
                company: Some(String::new()),
                emails: Some(vec![]),
                phones: Some(vec![]),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(cleared.display_name, renamed.display_name);
    assert_eq!(cleared.company, "");
    assert!(cleared.emails.is_empty());
    assert!(cleared.phones.is_empty());
    api.delete_contact(id).await.unwrap();
    assert!(
        !api.all(Priority::User)
            .await
            .unwrap()
            .iter()
            .any(|c| c.id == id)
    );
    assert!(matches!(
        api.delete_contact(id).await,
        Err(Error::NotFound(_))
    ));
}

#[tokio::test]
async fn editing_folder_contact_preserves_labels_phone_types_and_categories() {
    let mock = MockGraph::start().await.unwrap();
    let id = mock.mailbox("default").add_contact(Contact {
        folder_id: "friends".into(),
        display_name: "Chris".into(),
        email_addresses: vec![EmailAddress::new("Studio", "studio@example.com")],
        mobile_phone: Some("555-0100".into()),
        home_phones: vec!["555-0101".into()],
        business_phones: vec!["555-0102".into()],
        categories: vec!["Book club".into()],
        ..Default::default()
    });
    let client = common::client(&mock, "test-token");
    let api = ContactsApi::new(client.clone());
    api.save_contact(
        AccountId(1),
        Some(&id),
        &ContactChanges {
            emails: Some(vec![
                "studio@example.com".into(),
                "chris@example.com".into(),
            ]),
            phones: Some(vec![
                "555-0101".into(),
                "555-0103".into(),
                "555-0100".into(),
            ]),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let saved = client
        .get(&format!("/me/contacts/{id}"), Priority::User, &[])
        .await
        .unwrap();
    assert_eq!(saved["emailAddresses"][0]["name"], "Studio");
    assert_eq!(saved["mobilePhone"], "555-0100");
    assert_eq!(saved["homePhones"], serde_json::json!(["555-0101"]));
    assert_eq!(saved["businessPhones"], serde_json::json!(["555-0103"]));
    assert_eq!(saved["categories"], serde_json::json!(["Book club"]));
    assert_eq!(saved["parentFolderId"], "friends");
    api.delete_contact(&id).await.unwrap();
}
