// Where each registered command is reachable in the app. Every command must be
// listed: a new command in the registry fails `tsc` (and so CI) until it is
// given a place here. "palette" commands appear in the command palette with a
// form generated from their input schema.
import type { CommandName } from "../../../../../bindings/commands";

export type Surface = "palette" | "terminal";

export const COMMAND_SURFACE = {
  "agent.status": "palette",
  api: "palette",
  "audit.list": "palette",
  "auth.login": "terminal", // shows a device code and waits; run it in a terminal
  "auth.logout": "palette",
  "auth.status": "palette",
  "cal.add": "palette",
  "cal.calendars": "palette",
  "cal.delete": "palette",
  "cal.edit": "palette",
  "cal.list": "palette",
  "cal.propose": "palette",
  "cal.rsvp": "palette",
  "config.path": "palette",
  "config.validate": "palette",
  "contacts.search": "palette",
  "draft.create": "palette",
  "draft.forward": "palette",
  "draft.reply": "palette",
  "draft.send": "palette",
  "draft.update": "palette",
  "folder.create": "palette",
  "folder.delete": "palette",
  "folder.list": "palette",
  "folder.move": "palette",
  "folder.rename": "palette",
  "mail.archive": "palette",
  "mail.process": "palette",
  "mail.attachments": "palette",
  "mail.delete": "palette",
  "mail.fetch": "palette",
  "mail.flag": "palette",
  "mail.list": "palette",
  "mail.move": "palette",
  "mail.pin": "palette",
  "mail.read": "palette",
  "mail.search": "palette",
  "mail.send": "palette",
  "mail.thread": "palette",
  "mcp.serve": "terminal", // speaks MCP over stdio
  "ops.cancel": "palette",
  "ops.list": "palette",
  "ops.retry": "palette",
  schema: "palette",
  "sync.check": "palette",
  "sync.status": "palette",
  "ui.folder": "palette",
  "ui.open": "palette",
  version: "palette",
} as const satisfies Record<CommandName, Surface>;
