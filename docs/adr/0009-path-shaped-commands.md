# 0009. Path-shaped commands: the URL is the command

- **Status:** Accepted
- **Date:** 2026-11-09
- **Deciders:** Project owner
- **Supersedes:** the naming and argument rules of [ADR 0002](0002-command-surface.md). The rest of 0002 (API name first, reserved names, `--describe`, sending, exit codes) stands.

## Context

ADR 0002 turns each operation into `<resource…> <verb>` with semantic verbs: GET → `list`/`get`, POST → `create`, PUT → `replace`, and heuristics for actions and queries. That needs a translation table users must learn, and it misfires on RPC-style APIs, common in ASP.NET exports. `POST /api/v1/account/login` became `account login create`, and the Jci Identity API (700 operations) needed four hand-written `x-cli-name` patches before it imported at all. Path parameters were positional, so a command with several of them (`/tenantbranding/public/{tenantId}/{appKey}`) gave no hint which value went where.

## Options considered

1. **Keep 0002, tune the heuristics.** Each new API style needs more rules. The translation stays.
2. **HTTP verbs as the last word for every method** (`countries get`). One-to-one, but `GET /countries` and `GET /countries/{id}` both become `countries get` and collide.
3. **The URL is the command.** Path words form the command, GET has no word, other methods are named, and every input is a flag named after the spec. Operations that differ only by path parameters become one command whose flags pick the URL.

## Decision

Option 3, for every API (no per-API mode: two naming schemes is debt).

```
opencli <api> <path words…> [method] [--param value …] [--body …]
```

- **Command words are the path's fixed segments**, kebab-cased. Placeholders (`{id}`) leave the command and become flags. Leading `api`, `apis` and `rest` segments are stripped from each path (`/api/v1/users` → `v1 users`, `/health` → `health`); a clash that causes fails the import. Version segments (`v1`) stay, so importing a `v2` later adds commands instead of renaming them.
- **The method is the last word, except GET**: `post`, `put`, `patch`, `delete`. A command without a method word only ever reads.
- **Every input is a flag named after the spec**: path, query, header and cookie parameters (`{tenantId}` → `--tenant-id`) and scalar body fields. No positionals. Clashes with reserved flags or within an operation keep 0002's location prefix (`--query-output`, `--body-name`). Secret body fields are never flags.
- **Variants.** Operations with the same command words and method are one command; each is a *variant*, chosen by the path flags given:
  1. the variant whose path parameters are exactly the path flags given;
  2. otherwise, the only variant whose path parameters include them, with the rest asked for in a terminal or failing as missing;
  3. otherwise the choice is ambiguous: `-i` asks which, anything else is a usage error that lists the variants.

  A flag a variant doesn't take is a usage error naming that variant. When a path parameter in one variant and a query, header or cookie parameter in another share a name, the path parameter keeps it and the other takes its location prefix.
- **A resource word can be a command and a group at once.** `countries` reads; `countries post` and `countries cities` sit beneath it.
- **Writes that act on a whole collection say so.** When a write variant has fewer path parameters than a sibling variant, the confirmation shows `(all items)` beside the URL. Scripts still need `--yes`.
- **Hints.** `x-cli-resource` replaces the command words. `x-cli-name` replaces the method word (on a GET it adds one).
- **Conflicts fail the import**, listing the operations involved, with an `x-cli-name` hint. Two cases remain: two variants of a command with the same path parameters, and one set of command words used by different methods (`GET /blog/post` and `POST /blog`). Nothing is renamed silently. The operationId fallbacks of 0002 are gone: they existed to break ties the verb table created.
- **Discovery.** `--help` at any level lists what sits beneath it. `opencli api commands <api> [words…]` searches every command's words, method, path, summary and operationId (all words must match, ignoring case) and prints each match with its path flags and endpoint. It lives under `api` rather than beside an API's own commands, where a path word like `find` would clash with it.
- **`--describe`** prints `{"command": "...", "variants": [...]}`, one entry per variant, each in the per-operation shape 0002 defined.

| Spec | Command |
|---|---|
| `GET /api/v1/countries` | `countries` |
| `GET /api/v1/countries/{id}` | `countries --id 5` |
| `POST /api/v1/countries` | `countries post --body @c.json` |
| `DELETE /api/v1/countries/{id}` | `countries delete --id 5` |
| `GET /api/v1/countries/{id}/cities` | `countries cities --id 5` |
| `POST /api/v1/account/login` | `account login post -i` |

(Each prefixed by `opencli <api> v1`.)

## Consequences

- Commands read like the API's own documentation, with nothing to translate. RPC-style and REST-style APIs follow the same rule.
- The four Jci Identity collisions resolve without patches: different words, or variants of one command.
- Commands are slightly longer: `countries --id 5` instead of `countries get 5`.
- Breaking for existing users: command names, positional path parameters, plugin aliases and table formats keyed by old names (`"issues list"`), and the `--describe` shape all change. Caches rebuild on their own (cache format bump); stored APIs and credentials are untouched. Completion scripts must be regenerated.
- Specs patched with `x-cli-name` for 0002 keep their names, now as method words (`operationconfigurations delete-collection`). Re-importing the unpatched spec gives the plain names.
