# Contacts in Familiar

## Design

Add Contacts immediately after Calendar. Browse the saved contacts from every synced Outlook and Gmail account, filter by account, and search by name, company, email address or phone. Each list item represents one provider record. A person saved in two accounts has two independently editable entries. Include people who only have a phone number.

Show the account and contact details beside the list, with a compose action for each email address. New contacts require a destination account. Edit name, company, email addresses and phone numbers. Ask for confirmation before deleting from the named account. Keep the editor open when a provider refuses a change.

Familiar already caches both address books for autocomplete. Use that cache for browsing offline. The ranked recipient search also contains past correspondents and merges addresses, so it cannot identify the record to edit. Send writes through the daemon to the owning provider, and apply the provider response to the cache. Request contacts write access at sign-in. Existing users can sign in again if their provider refuses a write.

## Contact managers considered

- Outlook People keeps the list visible when you select a contact and gives editing a distinct action. Familiar uses that list-and-card layout so someone can check a phone number or start an email in one selection. [Outlook People](https://support.microsoft.com/en-us/outlook/create-view-and-edit-contacts-and-contact-lists-in-outlook)
- Google Contacts distinguishes saved people from automatically remembered email recipients. Familiar keeps past correspondents in compose suggestions and shows saved cards in Contacts. [Google Contacts](https://support.google.com/contacts/answer/1069522)
- Apple Contacts can link cards across services while retaining the underlying accounts. Google limits merging to a single account. Familiar shows the owning account on each card and scopes edits and deletion to that record, even when two records share an email address. [Apple Contacts](https://support.apple.com/en-gb/guide/contacts/adrb33f38d93/mac) and [Google duplicate handling](https://support.google.com/contacts/answer/7078226)

- Cardhop makes the next action available on the card and uses the standard New shortcut within its contacts view. Familiar opens compose from each address, uses Ctrl/Cmd+N for a new contact while Contacts is active, and puts focus in contact search with Ctrl/Cmd+F. [Cardhop actions](https://flexibits.com/cardhop/help/performing-actions) and [Cardhop shortcuts](https://flexibits.com/cardhop/tips)

## Implementation plan

1. Add a local list command returning accounts and complete saved contact records. Add provider-confirmed create, edit and delete commands with account checks and agent grants. Serialize writes and full sync per account so a stale sync cannot undo a successful save.
2. Implement Microsoft Graph contact mutations and Google People mutations. Send changed fields only when editing. Preserve provider data outside the edited fields and follow Google's contact-source concurrency rules. Test with disposable provider fixtures.
3. Add a searchable Contacts tab and keyboard-accessible list, details, account picker and editor. Compose through Familiar's existing new-message window. Keep mail shortcuts scoped to the mail view.
4. Update consent instructions and user documentation, regenerate the registry bindings and command reference, and verify the desktop build. Run the repository checks, review and merge the PR, check main CI, and install the matching desktop client and daemon.
