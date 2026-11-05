// Reads command input schemas (JSON Schema from `fam schema`) into form fields,
// and turns filled-in values back into command input and a `fam` command line.

export type FieldKind = "string" | "integer" | "number" | "boolean" | "list" | "enum" | "json";

export type Field = {
  name: string;
  kind: FieldKind;
  description: string;
  required: boolean;
  positional: number | null;
  /** For `enum`. */
  options: string[];
  /** For `list`: whether the items are numbers. */
  numericItems: boolean;
};

type Schema = {
  type?: string | string[];
  description?: string;
  enum?: unknown[];
  items?: Schema;
  $ref?: string;
  oneOf?: Schema[];
  anyOf?: Schema[];
  properties?: Record<string, Schema>;
  required?: string[];
  "x-positional"?: number;
  $defs?: Record<string, Schema>;
};

function resolve(s: Schema, root: Schema): Schema {
  if (s.$ref?.startsWith("#/$defs/")) {
    const def = root.$defs?.[s.$ref.slice("#/$defs/".length)];
    if (def) return { ...def, description: s.description ?? def.description };
  }
  // `Option<Enum>` comes out as anyOf [{$ref}, {type: null}].
  const alts = (s.anyOf ?? s.oneOf)?.filter((a) => a.type !== "null");
  if (alts?.length === 1 && alts[0]) {
    return { ...resolve(alts[0], root), description: s.description };
  }
  return s;
}

function kindOf(s: Schema, root: Schema): Pick<Field, "kind" | "options" | "numericItems"> {
  const r = resolve(s, root);
  if (r.enum?.every((v) => typeof v === "string")) {
    return { kind: "enum", options: r.enum as string[], numericItems: false };
  }
  const types = (Array.isArray(r.type) ? r.type : [r.type]).filter((t) => t !== "null");
  const t = types[0];
  if (t === "array") {
    const item = resolve(r.items ?? {}, root);
    const it = Array.isArray(item.type) ? item.type[0] : item.type;
    if (it === "string" || it === "integer" || it === "number") {
      return { kind: "list", options: [], numericItems: it !== "string" };
    }
    return { kind: "json", options: [], numericItems: false };
  }
  if (t === "string" || t === "integer" || t === "number" || t === "boolean") {
    return { kind: t, options: [], numericItems: false };
  }
  return { kind: "json", options: [], numericItems: false };
}

/** The form fields of an input schema: positional ones first, then required, then the rest. */
export function fieldsOf(input: unknown): Field[] {
  const root = (input ?? {}) as Schema;
  const required = new Set(root.required ?? []);
  const fields = Object.entries(root.properties ?? {}).map(([name, s]) => ({
    name,
    description: s.description ?? resolve(s, root).description ?? "",
    required: required.has(name),
    positional: s["x-positional"] ?? null,
    ...kindOf(s, root),
  }));
  const rank = (f: Field) => (f.positional !== null ? f.positional : f.required ? 100 : 200);
  return fields.sort((a, b) => rank(a) - rank(b) || a.name.localeCompare(b.name));
}

export type Values = Record<string, string | boolean>;

function number(field: string, v: string): number {
  const n = Number(v.trim());
  if (v.trim() === "" || Number.isNaN(n)) throw new Error(`${field.replace(/_/g, " ")}: \`${v}\` is not a number`);
  return n;
}

/** Command input from form values. Empty fields are left out. */
export function inputOf(fields: Field[], values: Values): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const f of fields) {
    const v = values[f.name];
    if (v === undefined || v === "" || v === false) continue;
    if (f.kind === "boolean") out[f.name] = v === true;
    else if (typeof v !== "string") continue;
    else if (f.kind === "integer" || f.kind === "number") out[f.name] = number(f.name, v);
    else if (f.kind === "list") {
      // Commas inside quotes or <...> stay: `"Doe, Jane" <jane@example.com>`.
      const items = (v.match(/(?:"[^"]*"|<[^>]*>|[^,])+/g) ?? []).map((x) => x.trim()).filter(Boolean);
      out[f.name] = f.numericItems ? items.map((x) => number(f.name, x)) : items;
    } else if (f.kind === "json") out[f.name] = JSON.parse(v);
    else out[f.name] = v;
  }
  return out;
}

const quote = (s: string) => (/^[\w@.,:/+-]+$/.test(s) ? s : `"${s.replace(/(["\\$`])/g, "\\$1")}"`);

/** The `fam` command line for `cli` (such as `fam mail archive`) with this input. */
export function cliLine(cli: string, fields: Field[], input: Record<string, unknown>): string {
  const parts = [cli];
  const flag = (n: string) => `--${n.replace(/_/g, "-")}`;
  for (const f of fields) {
    const v = input[f.name];
    if (v === undefined) continue;
    const vals = Array.isArray(v) ? v.map(String) : [typeof v === "object" ? JSON.stringify(v) : String(v)];
    if (f.positional !== null) parts.push(...vals.map(quote));
    else if (f.kind === "boolean") parts.push(flag(f.name));
    else for (const x of vals) parts.push(flag(f.name), quote(x));
  }
  return parts.join(" ");
}
