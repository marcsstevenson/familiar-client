# Google sign-in test guide (Gmail plan WP G.0)

About 15 minutes. Part 1 creates Familiar's Google project in your own Google account. Part 2 runs a small test program that signs in to your Gmail and checks Familiar can reach mail, and later calendar and contacts. Nothing is changed in your mailbox.

Do it all signed in as your `@gmail.com` account.

## Part 1: Create the Google project

1. Go to <https://console.cloud.google.com> and accept the terms if asked. You don't need a billing account or a free trial: these APIs cost nothing.
2. Click the project picker at the top left, then **New project**. Name it `Familiar`, leave **Location** as *No organization*, and click **Create**. Make sure `Familiar` is selected in the picker afterwards.
3. Turn on the three APIs. For each one, open the link, check the `Familiar` project is selected, and click **Enable**:
   - Gmail API: <https://console.cloud.google.com/apis/library/gmail.googleapis.com>
   - Google Calendar API: <https://console.cloud.google.com/apis/library/calendar-json.googleapis.com>
   - People API: <https://console.cloud.google.com/apis/library/people.googleapis.com>

   Calendar and contacts come later, but turning them on now means one sign-in covers everything.
4. Set up the consent screen. Open <https://console.cloud.google.com/auth/overview> and click **Get started**:
   - **App name:** `Familiar`. **User support email:** your Gmail address.
   - **Audience:** *External*.
   - **Contact information:** your Gmail address.
   - Agree to the policy and click **Create**.
5. Publish it. Open **Audience** (left menu) and, under **Publishing status**, click **Publish app**, then **Confirm**. The status must read **In production**.

   This step matters. Left in *Testing*, Google signs you out every 7 days. Google may say the app needs verification. Ignore that and don't submit anything: verification is only for apps offered to the public, and an unverified app works for you after a warning screen.
6. Add the scopes. Open **Data access** and click **Add or remove scopes**. At the bottom, under **Manually add scopes**, paste this, click **Add to table**, then **Update** and **Save**:

   ```
   https://www.googleapis.com/auth/gmail.modify
   https://www.googleapis.com/auth/gmail.send
   https://www.googleapis.com/auth/gmail.settings.basic
   https://www.googleapis.com/auth/calendar
   https://www.googleapis.com/auth/contacts.readonly
   https://www.googleapis.com/auth/contacts.other.readonly
   ```

   The table sorts them into non-sensitive, sensitive and restricted. Note which group `gmail.settings.basic` lands in. That's one of the test's questions.
7. Create the client. Open **Clients**, click **Create client**, choose **Application type: Desktop app**, name it `Familiar desktop`, and click **Create**.
8. In the box that appears, click **Download JSON**. Save the file as:

   ```
   C:\Users\marcs\.familiar\google-client.json
   ```

   Don't paste its contents into a chat. Google says a desktop app's client secret isn't really secret, but it still has no reason to leave your PC.

## Part 2: Run the test

In PowerShell:

```powershell
cd D:\git\marcsstevenson\familiar-client
git pull
cd spikes\google-auth
cargo run
```

A browser window opens:

1. Choose your Gmail account.
2. Google shows **Google hasn't verified this app**. That's expected. Click **Advanced**, then **Go to Familiar (unsafe)**. The warning is about Google not having audited the app, which only matters for apps offered to other people. The app is your own.
3. The next screen lists what Familiar wants. Tick **Select all** (or every box), then **Continue**.
4. The browser says you can close the window. Go back to PowerShell.

The program prints one line per check, such as:

```
sign-in            ok      granted: openid ... gmail.modify gmail.send ...
profile            ok      HTTP 200
                           messages: 12345, threads: 6789
labels             ok      HTTP 200
...
refresh            ok      granted: ...
```

It prints counts only, never names, addresses or subjects. Tokens stay in memory and are never saved.

## What to send back

- The program's whole output. Counts are fine to share.
- Which group `gmail.settings.basic` landed in (step 6).
- Whether the warning screen looked as described, or something else appeared, such as "This app is blocked". A screenshot helps if anything was different.

## Afterwards

Nothing to clean up. Familiar's real sign-in (WP G.3) reuses the same project and client file. To remove the test's access, go to <https://myaccount.google.com/connections>, find Familiar and remove it. The next sign-in asks again.
