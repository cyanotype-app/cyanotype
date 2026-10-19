# Administration

Cyanotype is multi-user. One or more accounts are **admins**; everyone else is a **member**. Admins manage who may
join. They never see other people's photos: the admin area deals in accounts, invites and counts, not library content.

This page describes what exists so far: roles, invites and the audit log. It grows with the portal.

## Roles

- `users.role` is `admin` or `member`.
- **Upgrading:** migration 0016 makes the earliest-created account an admin (one row updated). A single-owner instance
  therefore upgrades with its owner as admin and nobody locked out.
- **Fresh install:** the first account to register (in any mode but `closed`) becomes the admin; the check and the
  insert run under one advisory lock, so a race produces exactly one admin.
- `GET /auth/me` reports the role. The web app shows an **Admin** link to admins and treats `/admin` as an unknown page
  for members.
- **The server enforces it.** Admin endpoints (`/auth/admin/*`) use the `AdminUser` extractor
  (`crates/cyanotype-service/src/auth.rs`): it needs a **login session** (an API key is refused with 403, even an
  admin's, so a leaked CLI key cannot reach the portal), reads the role from the database on every request (a demotion
  takes effect at once), and answers 401 without a session and 403 for a member. Any service in the workspace can use it.
- Lost the only admin? Promote an account by SQL (see `docs/deploy.md`, "Upgrading an existing instance").

## Registration modes

`REGISTRATION_MODE` (auth service; `docs/deploy.md`):

| Mode | Who can register |
|---|---|
| `invite` (default) | The first account of an empty instance, freely. Everyone after that only with a valid invite token. |
| `first_user` | The first account only. Invites are ignored. |
| `open` | Anyone. A valid invite token, if given, is still recorded as used. |
| `closed` | Nobody (403). |

`GET /auth/registration` tells the register page the mode and whether the instance still has no account.

## Invites

An admin makes an invite on **Admin > Invites** (`POST /auth/admin/invites`):

- **Single use.** The first registration that presents the token claims it atomically (a row lock), so ten people racing
  with one link register exactly one account. A registration that then fails (the email is taken) rolls the claim back and
  the invite stays usable.
- **Expiry**, 7 days by default, 1 to 90 days.
- **Note**, only the admin sees it (up to 200 characters).
- **Preset limits** (storage, uploads per day, largest file; all optional, positive byte counts) are stored on the invite
  for the limits work to copy onto the new account.
- At most 100 invites may be waiting at once.

The link is `https://<your host>/register?invite=<token>`. The server sends no email: the admin copies the link and sends
it. The register page reads `?invite=` (and, on an invite-only server without one, accepts a pasted link or token), checks
it with `POST /auth/invites/check` and says up front when it was used, revoked or has expired.

**Tokens are stored hashed** (sha256 of 32 random bytes). The plaintext exists only in the response that creates or
re-issues an invite. So "show the link again" means **re-issue**: `POST /auth/admin/invites/{id}/reissue` replaces the
token (the old link stops working immediately) and returns the new one. The expiry is kept unless the invite has already
expired or a new `expires_in_days` is given.

Other actions: list with status (`pending`, `used`, `expired`, `revoked`) and who used each one; **revoke**
(`DELETE /auth/admin/invites/{id}`, final, idempotent, not allowed once used).

## Audit log

`audit_log` records admin actions: when, the actor (id and email, kept after the account is deleted), an action name, the
target and a small JSON detail. Never a token, a password or a photo name. Written so far: `invite.create`,
`invite.reissue`, `invite.revoke` (by the admin) and `invite.use` (by the new account). Services write entries with
`cyanotype_service::audit::record`.
