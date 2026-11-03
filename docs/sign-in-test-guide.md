# Sign-in test guide (WP 0.1 and WP 0.2 step A)

About 15 minutes. Part 1 registers the Familiar app in your own Azure directory, so it doesn't depend on Parklands. Part 2 checks whether your iM3 Dental account can use it.

## Part 1: Register the app (signed in with the account that owns your Azure subscription)

1. Go to <https://entra.microsoft.com> and sign in with your personal Azure account. Check the directory shown top right is your own (often "Default Directory"), not Parklands or iM3. Switch with the settings cog > **Directories + subscriptions** if needed.
2. Open **Identity > Applications > App registrations > New registration**.
   - **Name:** `Familiar`
   - **Supported account types:** *Accounts in any organizational directory (Any Microsoft Entra ID tenant - Multitenant) and personal Microsoft accounts (e.g. Skype, Xbox)*
   - **Redirect URI:** choose platform **Public client/native (mobile & desktop)** and enter `http://localhost`
   - Click **Register**.
3. On the app's **Overview** page, copy the **Application (client) ID**. This is the value you send back. It is public, not a secret. Also note the **Directory (tenant) ID** for the record.
4. Open **Authentication**. Under **Advanced settings**, set **Allow public client flows** to **Yes**, then **Save**.
5. Open **API permissions > Add a permission > Microsoft Graph > Delegated permissions** and tick:
   - `User.Read` (usually already there)
   - `offline_access`
   - `Mail.ReadWrite`
   - `Mail.Send`
   - `Calendars.ReadWrite`
   - `Contacts.Read`
   - `MailboxSettings.ReadWrite`

   Click **Add permissions**.
6. You can skip **Grant admin consent** in your own directory. No one else signs in there.
7. Grant consent for Parklands, so you and anyone at the club can sign in without a prompt. In a private window, open this link with `CLIENT_ID` replaced, sign in as your **Parklands admin**, and accept:

   ```
   https://login.microsoftonline.com/organizations/v2.0/adminconsent?client_id=CLIENT_ID&scope=https%3A%2F%2Fgraph.microsoft.com%2F.default&redirect_uri=http%3A%2F%2Flocalhost
   ```

   The browser ends on a `localhost` page that won't load. If the address bar shows `admin_consent=True`, it worked. This step is optional for the iM3 test, so it can wait.

Don't create a client secret or certificate. Familiar is a public client and doesn't need one.

## Part 2: iM3 Dental consent probe (signed in as your iM3 user)

1. Open a **private/InPrivate browser window**, so no Parklands or Hotmail session gets in the way.
2. Take the link below, replace `CLIENT_ID` with the ID from Part 1 step 3, and open it. (Or send the ID to Claude and you'll get the links back ready to click.)

   **Full scopes:**

   ```
   https://login.microsoftonline.com/organizations/oauth2/v2.0/authorize?client_id=CLIENT_ID&response_type=code&redirect_uri=http%3A%2F%2Flocalhost&response_mode=query&scope=openid%20profile%20offline_access%20User.Read%20Mail.ReadWrite%20Mail.Send%20Calendars.ReadWrite%20Contacts.Read%20MailboxSettings.ReadWrite&prompt=select_account
   ```

3. Sign in as your **iM3 Dental** user and see which screen you get:

   | What you see | Meaning | What to do |
   | --- | --- | --- |
   | A "Permissions requested" screen listing the permissions | You can consent yourself, so iM3 will work | Click **Accept**. The browser then goes to a `localhost` page that won't load. That's expected. Check the address bar starts with `http://localhost/?code=`. Report "consent OK". Don't paste the code. |
   | "Need admin approval" | User consent is blocked for this app | Don't submit an approval request yet. Report it and try the read-only link below. |
   | "Your sign-in was blocked", "You can't get there from here", or an error code like `AADSTS53003` | A Conditional Access policy blocks it | Note the error code and the Correlation ID from **More details**, and report them. |
   | The address bar shows `http://localhost/?error=...` | Something else refused it | Copy the `error` and `error_description` parts (not any `code`) and report them. |

4. **Only if the full scopes were refused**, close the private window, open a new one, and try the read-only set:

   ```
   https://login.microsoftonline.com/organizations/oauth2/v2.0/authorize?client_id=CLIENT_ID&response_type=code&redirect_uri=http%3A%2F%2Flocalhost&response_mode=query&scope=openid%20profile%20offline_access%20User.Read%20Mail.Read%20Calendars.Read&prompt=select_account
   ```

   Report what you see, using the same table.

## Good to know

- If you accept, iM3's directory gets an "enterprise app" entry for Familiar under your user. An iM3 admin could see it. You can remove your consent at any time from <https://myapps.microsoft.com> (your iM3 account, then **Manage your applications**) or <https://myaccount.microsoft.com>.
- Nothing is read or sent at this stage. The probe only shows whether consent is possible. Reading mail starts with the token spike (step B), which runs on your computer.
- Next if consent is blocked: device code retry, then a one-page request to the iM3 admin that Claude drafts and you decide whether to send. See WP 0.2 fallbacks in `designs/Development Plan.md`.
