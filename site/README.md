# familiarclient.com

The static homepage and privacy policy for Familiar, served by Cloudflare Pages
(project `familiar-site`). No build step: the files here are the site.

```sh
npx wrangler pages deploy site --project-name familiar-site --branch main
```

The privacy policy is also the one Google's OAuth consent screen links to, so
keep it in step with the scopes in `designs/Gmail Plan.md` and with what
`docs/ai.md` and `docs/spam.md` say is sent to an AI tool.
