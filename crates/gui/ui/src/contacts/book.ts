import type { Contact } from "./ContactEditor";

export const title = (c: Contact) => c.name || c.company || c.emails[0] || c.phones[0] || "Unnamed contact";
export const key = (c: Contact) => JSON.stringify([c.account, c.remote_id]);
export const target = (c: Contact) => ({ account: c.account, contact: c.id, remote_id: c.remote_id });
export const snapshot = (c: Contact) => ({
  name: c.name, company: c.company, emails: c.emails, phones: c.phones, starred: c.starred,
});

export function googleLink(c: Contact) {
  const person = /^people\/([^/]+)$/.exec(c.remote_id)?.[1];
  if (person && c.google_account) {
    return `https://contacts.google.com/person/${encodeURIComponent(person)}?authuser=${encodeURIComponent(c.google_account)}`;
  }
}

function phoneKey(phone: string) {
  // Ignore presentation punctuation, retaining extensions and international prefixes.
  return phone.trim().toLowerCase().replace(/[\s().-]/g, "");
}

export function duplicateGroups(contacts: Contact[]) {
  const groups = new Map<string, { reason: string; cards: Contact[] }>();
  for (const c of contacts) {
    const add = (type: string, value: string) => {
      if (!value) return;
      const id = JSON.stringify([c.account, type, value]);
      const group = groups.get(id) ?? { reason: value, cards: [] };
      if (group.cards.at(-1) !== c) group.cards.push(c);
      groups.set(id, group);
    };
    add("name", c.name.trim().toLowerCase().replace(/\s+/g, " "));
    for (const email of c.emails) add("email", email.trim().toLowerCase());
    for (const phone of c.phones) {
      if (phone.replace(/\D/g, "").length >= 7) add("phone", phoneKey(phone));
    }
  }
  return [...groups.values()].filter(group => group.cards.length > 1);
}

function unique(values: string[], normalise: (value: string) => string) {
  const seen = new Set<string>();
  return values.filter(value => {
    const id = normalise(value);
    if (!id || seen.has(id)) return false;
    seen.add(id);
    return true;
  });
}

export function mergeFields(keep: Contact, cards: Contact[]) {
  const ordered = [keep, ...cards.filter(c => key(c) !== key(keep))];
  return {
    name: ordered.find(c => c.name)?.name ?? "",
    company: ordered.find(c => c.company)?.company ?? "",
    emails: unique(ordered.flatMap(c => c.emails), s => s.trim().toLowerCase()),
    phones: unique(ordered.flatMap(c => c.phones), phoneKey),
    starred: ordered.some(c => c.starred),
  };
}

export function includesMerge(saved: Contact, fields: ReturnType<typeof mergeFields>) {
  const emails = new Set(saved.emails.map(e => e.trim().toLowerCase()));
  const phones = new Set(saved.phones.map(phoneKey));
  return (!fields.name || saved.name === fields.name) && (!fields.company || saved.company === fields.company)
    && (!fields.starred || saved.starred) && fields.emails.every(e => emails.has(e.trim().toLowerCase()))
    && fields.phones.every(p => phones.has(phoneKey(p)));
}
