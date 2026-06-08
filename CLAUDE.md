## AXREG FORMAT — strict rules, no exceptions

Every `fn` entry in an axreg file has exactly these fields in this order:

```
fn <name>
  identity <0xHASH>
  kind     leaf | composite
  in       (<Type>, <Type>, ...)   ← comma-separated, parenthesised
  out      <Type>
  effect   pure | reads | writes | fullIo
  deterministic true | false
  idempotent    true | false
end
```

### VALID TYPE NAMES — only these, no others

```
Int  Text  Bool  Unit  TextList  ResultText  ResultUnit  Value
```

### FORBIDDEN FIELDS — never add these

| Field    | Why forbidden |
|----------|--------------|
| `arity`  | Not a real axreg field. Arity is derived from `in (...)` by counting types. |
| `profile`| Wrong keyword. The correct keyword is `effect`. |

### FORBIDDEN ACTIONS on axreg files

- Never remove or modify the `identity` field of any entry.
- Never add fields not in the list above.
- Never use type names outside the valid list.
- Never use `profile` — use `effect`.
- Never add `arity` — it is not a valid field.

### When adding a new function

1. Add `in (...)`, `out`, `effect` using types from the valid list only.
2. Derive the identity hash using `registry_compound_id(name, contract)`.
3. Do not invent an identity hash.
4. If the correct type cannot be determined from the Rust source,
   leave the entry without `in`/`out` and report it as a gap.
