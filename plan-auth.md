# Optional username/password accounts

Add durable accounts on top of the existing UUID-based guest model. Guest mode keeps working exactly as it does today. Registered accounts are bound to a session cookie and can sign in from any browser.

## Decisions locked in

- **Identity stays keyed on `player_id` UUID** everywhere. Username/password live on `player_profiles`; they are *attached to* an existing UUID, not a new identity.
- **Registering preserves guest progress.** The guest's existing `player_id`, runs, leaderboard rows, profile, etc. all stay intact — we just attach credentials to that UUID.
- **Session cookie is read at WebSocket upgrade.** Once verified, the connection is bound to that `player_id`; the body's `player_id` field is *ignored* for registered accounts and re-derived from the session.
- **Guest behavior unchanged.** No cookie + no username on profile = today's flow.
- **Case-insensitive usernames** (`Patrick` and `patrick` collide).
- **Sliding session expiry**, bumped on each authenticated request.
- **Out of scope**: password reset, email, 2FA, account deletion.

## Schema (additions in `src/db.rs::Db::open`, behind a `PRAGMA user_version` bump)

```sql
ALTER TABLE player_profiles ADD COLUMN username TEXT;
ALTER TABLE player_profiles ADD COLUMN password_hash TEXT;
CREATE UNIQUE INDEX idx_profiles_username
  ON player_profiles(username COLLATE NOCASE)
  WHERE username IS NOT NULL;

CREATE TABLE sessions (
  token TEXT PRIMARY KEY,            -- 32 random bytes, hex
  player_id TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,       -- 24h default; 30d if "stay signed in"
  last_seen_at INTEGER NOT NULL
);
CREATE INDEX idx_sessions_player ON sessions(player_id);
```

Username/password live on `player_profiles` rather than a new `users` table — `player_id` is already the PK everywhere downstream. Multiple sessions per account fall out of `sessions` being its own table.

## Auth model

| State | Detected by | Behavior |
|---|---|---|
| **Guest** | `username IS NULL` for player_id | Identity = `player_id` from WS message body. No cookie required. Today's flow. |
| **Registered, signed in** | Valid session cookie | Identity = `session.player_id`. Body `player_id` ignored. |
| **Registered, no session** | `username IS NOT NULL` and no/expired cookie | WS messages for that player rejected with `auth_required`. Edge-case profile screen on the client. |

Cookie: `vs_session=<token>`, `HttpOnly`, `SameSite=Lax`, `Secure` when behind TLS, `Path=/`. Lifetime: 24h sliding by default; 30d sliding if "stay signed in" was checked at login.

## New crates

- `argon2` — password hashing (pure Rust, modern default).
- Existing `rand` — `OsRng` for token generation.

No cookie crate; we'll parse the `Cookie` header and emit `Set-Cookie` manually (one or two short helpers).

## New HTTP endpoints (alongside `/ws`)

All under `/api/`. JSON in/out.

- `POST /api/register` — body `{ player_id, username, password }`. Attaches creds to the existing guest profile (which `player_id` references). Errors: `username_taken`, `username_invalid`, `password_too_short`, `already_registered`. On success creates session, sets cookie, returns `{ ok: true }`.
- `POST /api/login` — body `{ username, password, stay }`. Errors: `invalid_credentials`, `rate_limited`. On success creates session, sets cookie, returns `{ player_id }`.
- `POST /api/logout` — deletes the cookie's session row, clears cookie.
- `GET /api/whoami` — reads cookie, returns `{ player_id, username | null, has_account, signed_in }`. Used by the client on boot to pick the right screen.

Game actions stay on the WebSocket.

## Validation rules

- **Username**: 3–24 chars, `[a-zA-Z0-9_-]`, case-insensitive uniqueness via `COLLATE NOCASE` index. Display preserves the case the user typed; lookups use NOCASE.
- **Password**: min 6 chars. No max beyond argon2's input limit.
- **Confirm-password**: enforced client-side only (server takes one password).

## Rate limiting

Crude in-memory token bucket per IP, 10 attempts/min, applied to `/api/login` and `/api/register`. State: `Arc<Mutex<HashMap<IpAddr, Bucket>>>` on `AppState`. Returns 429 + `rate_limited`.

## WebSocket integration

- On upgrade: parse `Cookie` header, look up session, attach `Option<AuthedPlayer>` to the connection.
- Per message: if the message's `player_id` resolves to a profile with `username IS NOT NULL`, require an `AuthedPlayer` whose `player_id` matches. If missing → reply `{ type: "auth_required" }`. If mismatched → same.
- Guest `player_id`s pass through unchanged.
- Sessions get `last_seen_at`/`expires_at` bumped on a successful authed message (sliding expiry).

Login/logout/register on the client happen over HTTP; afterwards the client drops and reconnects the WS so the upgrade picks up the new cookie state.

## Frontend changes

### Boot flow (`static/main.js`)

On load, before connecting WS: `fetch('/api/whoami')`. Decide:

1. **No `username` on player record** → guest flow. Start screen shows **New run** + **Log in**.
2. **`username` present + `signed_in: true`** → registered flow. Same as guest, but profile pill shows username, Register button hidden, **Log out** appears.
3. **`username` present + `signed_in: false`** → edge-case screen. Replace the start-screen "New run" button with a profile card (avatar + display name + username) and an inline login form (prefilled username, focus password). Below it, a small *"Sign in as another user"* link that calls `/api/logout` (no-op server-side if no cookie), wipes `localStorage.playerUuid`, and reloads — landing the user in the **fresh-guest** state with a brand-new UUID. (This is *not* deleting a guest, since the local UUID belonged to a registered account this device wasn't authed for.)

### Profile modal (`static/index.html` + `static/main.js`)

- Add a **Register** button (visible only when `has_account` is false).
- Clicking opens an inline panel inside the modal: username, password, confirm password, prominent warning ("we cannot recover this — write it down"), Cancel + Register buttons.
- After successful register: close the modal, refresh whoami state, swap the Register button for **Log out**.

### Login screen

New `<section id="login">` in `static/index.html`. Username, password, "stay signed in" checkbox, Sign in button, Cancel/back link.

### After register/login

- Set `localStorage.playerUuid` to the returned `player_id` (for login) / keep current (for register).
- Drop the WS, reconnect. Server upgrade now sees the cookie.

## Implementation steps

1. **Crates**: add `argon2` to `Cargo.toml`.
2. **Schema** (`src/db.rs`): bump `user_version`, run `ALTER TABLE`s + `CREATE TABLE sessions` + indexes inside the version migration.
3. **DB layer** (`src/db.rs`):
   - `attach_credentials(player_id, username, password_hash) -> Result<(), AttachErr>` — fails on `UsernameTaken` / `AlreadyRegistered`.
   - `find_account_by_username(username) -> Option<(player_id, password_hash)>` (NOCASE).
   - `create_session(player_id, ttl_seconds) -> token`.
   - `lookup_session(token) -> Option<(player_id, expires_at)>` — returns `None` if expired (and deletes it).
   - `bump_session(token, ttl_seconds)`.
   - `delete_session(token)`.
   - `profile_auth_status(player_id) -> { username: Option<String>, has_account: bool }`.
4. **Auth helpers** (`src/auth.rs`, new):
   - `hash_password(pw) -> String` (argon2id).
   - `verify_password(pw, hash) -> bool`.
   - `gen_session_token() -> String` (32 random bytes, hex).
   - Username/password validators.
5. **Cookie helpers** (`src/auth.rs` or `src/main.rs`):
   - `parse_session_cookie(headers: &HeaderMap) -> Option<&str>`.
   - `set_session_cookie(token, max_age) -> HeaderValue`, `clear_session_cookie() -> HeaderValue`.
6. **Rate limiter** (`src/auth.rs`): per-IP bucket on `AppState`.
7. **HTTP routes** (`src/main.rs`):
   - `POST /api/register`, `POST /api/login`, `POST /api/logout`, `GET /api/whoami`.
   - Wire IP extraction (`ConnectInfo<SocketAddr>`).
8. **WS upgrade** (`src/main.rs`): read cookie, resolve to `Option<AuthedPlayer>`, pass into the socket task.
9. **WS message gating** (`src/main.rs`): for any message with a `player_id` whose profile has a username, require an `AuthedPlayer` matching it. Otherwise emit `{ type: "auth_required" }` and return early.
10. **Frontend** (`static/index.html`, `static/main.js`):
    - Whoami fetch on boot.
    - Login screen markup + handlers.
    - Profile modal: Register panel + Log out button.
    - Edge-case start-screen variant.
    - Reconnect WS after auth state changes.

## Risks / things to watch

- **Argon2 timing**: hashing is intentionally slow. Run `verify_password` on a blocking thread (`tokio::task::spawn_blocking`) to avoid stalling the runtime under concurrent logins.
- **Cookie + WebSocket on different origins**: not relevant here (single origin), but `SameSite=Lax` is fine for WS upgrades from the same site.
- **Migration safety**: `ALTER TABLE ... ADD COLUMN` is non-destructive; existing rows get `NULL`s. `CREATE UNIQUE INDEX ... WHERE username IS NOT NULL` permits the existing all-NULL rows without conflict.
- **Session sweeping**: expired rows accumulate. Add a one-shot `DELETE FROM sessions WHERE expires_at < ?` at boot; revisit a periodic sweep if it ever matters.
- **Race on register**: two simultaneous registers of the same username — the unique index will reject the second with a constraint error; map that to `username_taken`.
