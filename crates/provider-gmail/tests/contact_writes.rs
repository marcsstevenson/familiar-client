// SPDX-License-Identifier: MIT OR Apache-2.0
//! People API writes against a disposable mailbox.

use std::sync::Arc;

use familiar_mock_gmail::MockGmail;
use familiar_provider_gmail::{ClientOptions, Error, GmailApi, GmailClient, Priority, StaticToken};
use familiar_store::{AccountId, ContactChanges};
use reqwest::Method;
use serde_json::json;

#[tokio::test]
async fn create_edit_clear_and_delete_a_contact() {
    let mock = MockGmail::start().await.unwrap();
    let opts = ClientOptions::with_base(format!("{}/users/me", mock.api_url()));
    let client = GmailClient::new(opts, Arc::new(StaticToken("test-token".into()))).unwrap();
    let api = GmailApi::new(client);
    let account = AccountId(1);
    let contact = api
        .save_contact(
            account,
            None,
            &ContactChanges {
                name: Some("Disposable Contact".into()),
                company: Some("Company".into()),
                emails: Some(vec!["test@example.org".into()]),
                phones: Some(vec!["+15555550100".into()]),
            },
        )
        .await
        .unwrap();
    assert_eq!(contact.account, account);
    assert_eq!(contact.display_name, "Disposable Contact");
    assert_eq!(contact.emails[0].addr, "test@example.org");
    assert_eq!(contact.phones, ["+15555550100"]);
    assert_eq!(contact.company, "Company");
    let remote = &contact.remote_id;
    let read_path = format!(
        "/{remote}?personFields=names,emailAddresses,phoneNumbers,organizations,biographies,metadata"
    );
    let read = || {
        api.client()
            .people_request(Method::GET, &read_path, None, Priority::User)
    };
    // Simulate rich data saved in Google Contacts that Familiar doesn't edit.
    let mut person = read().await.unwrap();
    let original_metadata = person["metadata"].clone();
    person["names"][0]["givenName"] = json!("Disposable");
    person["emailAddresses"][0]["type"] = json!("work");
    person["organizations"][0]["title"] = json!("Manager");
    person["biographies"] = json!([{"value":"Keep this note"}]);
    mock.mailbox("default").set_contacts(vec![person]);

    let edited = api
        .save_contact(
            account,
            Some(remote),
            &ContactChanges {
                company: Some("Renamed Company".into()),
                emails: Some(vec!["test@example.org".into(), "new@example.org".into()]),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(edited.company, "Renamed Company");
    assert_eq!(edited.display_name, contact.display_name);
    assert_eq!(edited.phones, contact.phones);
    assert_eq!(edited.emails.len(), 2);
    let after = read().await.unwrap();
    assert_eq!(after["names"][0]["givenName"], "Disposable");
    assert_eq!(after["emailAddresses"][0]["type"], "work");
    assert_eq!(after["organizations"][0]["title"], "Manager");
    assert_eq!(after["biographies"][0]["value"], "Keep this note");
    assert_ne!(after["metadata"], original_metadata);
    let requests = mock.requests();
    let patch = requests.iter().find(|r| r.method == "PATCH").unwrap();
    let query = reqwest::Url::parse(&format!("http://localhost/?{}", patch.query)).unwrap();
    let (_, mask) = query
        .query_pairs()
        .find(|(key, _)| key == "updatePersonFields")
        .unwrap();
    let mut mask = mask.split(',').collect::<Vec<_>>();
    mask.sort_unstable();
    assert_eq!(mask, ["emailAddresses", "organizations"]);
    assert!(requests.iter().any(|r| r.method == "GET"
        && r.query.contains("metadata")
        && r.query.contains("sources=READ_SOURCE_TYPE_CONTACT")));

    let cleared = api
        .save_contact(
            account,
            Some(remote),
            &ContactChanges {
                company: Some(String::new()),
                emails: Some(vec![]),
                phones: Some(vec![]),
                name: Some("Renamed Contact".into()),
            },
        )
        .await
        .unwrap();
    assert_eq!(cleared.display_name, "Renamed Contact");
    assert!(cleared.company.is_empty());
    assert!(cleared.emails.is_empty());
    assert!(cleared.phones.is_empty());
    assert_eq!(
        read().await.unwrap()["organizations"][0]["title"],
        "Manager"
    );

    api.delete_contact(remote).await.unwrap();
    assert!(matches!(read().await, Err(Error::NotFound(_))));
}
