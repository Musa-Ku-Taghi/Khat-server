# WebSocket + HTTP File Server Protocol (KHAT Server)

## Overview

This is an AIO server + config.json file for the KHAT android app (there will be desktop clients soon).

The server package consists of:
- **WebSocket** — for authentication, messaging, and file upload/download requests.
- **HTTP** — the upload/download and thumbnail retrieval protocol.

All HTTP endpoints require a Bearer token obtained from `signup`/`signin` (resets on server restart).

---

## WebSocket Protocol
Default addr: `ws://server:8765`

### General Message Format

All messages are JSON objects.
Every **request** from the client contains a `type` field identifying the action.
Every **response** from the server contains `type` (often `*_response`) and a `status` string.

Optional fields that are absent are **omitted**, not sent as `null`, unless explicitly noted otherwise below.

---

### 1. Sign Up

#### Request (Client ↦ Server)
```json
{
  "type": "signup",
  "iccid": "8998118315766229924",
  "username": "user",
  "password": "password123"
}
```
Rules:
- `iccid` — 19 or 20 digits, Luhn‑validated, starts with `89` + allowed country code (configurable).
- `username` — between 1 and `max_username_length` (configurable).
- `password` — between `min_password_length` and `max_password_length`.

#### Response (Server ↦ Client)
Success:
```json
{
  "type": "signup_response",
  "status": "success",
  "token": "a1b2c3d4e5f6..."
}
```
- The `token` must be used as Bearer token for HTTP endpoints.

Error status values:
- `iccid_error` — ICCID failed validation, or already exists
- `username_error` — username already exists
- `invalid_input`
- `error`

Note: `iccid_invalid` is **not** a status value — both "invalid ICCID" and "ICCID already exists" surface as `iccid_error`.

---

### 2. Sign In

#### Request (Client ↦ Server)
```json
{
  "type": "signin",
  "username": "user",
  "password": "password123"
}
```

#### Response (Server ↦ Client)
Success:
```json
{
  "type": "signin_response",
  "status": "success",
  "token": "a1b2c3d4e5f6..."
}
```
Error status values:
- `username_error` (not found)
- `password_error` (invalid password)
- `invalid_input`
- `error`

---

### 3. Search Users

#### Request (Client ↦ Server)
```json
{
  "type": "search_user",
  "username": "ali"
}
```
- `username` — substring to search (case‑insensitive). Results are ordered by earliest match position, then alphabetically.

#### Response (Server ↦ Client)
```json
{
  "type": "search_user_response",
  "status": "success",
  "results": [
    {
      "username": "QolamAli",
      "online": true,
      "profile_picture_url": "/profile_pics/42"
    },
    {
      "username": "alibaba",
      "online": false,
      "profile_picture_url": null
    }
  ]
}
```
- `online` — true if the user has at least one active WebSocket connection.
- `profile_picture_url` — URL of the user's primary profile picture (`/profile_pics/{file_id}`), or `null` if the user has none. Key is always present.
- Results limited by `search_results_limit` from config.

---

### 4. Send Message

#### Request (Client ↦ Server)
```json
{
  "type": "send_message",
  "recipient": "mmas",
  "content": [
    { "type": "text", "text": "1300" },
    {
      "type": "file",
      "file_id": 42,
      "name": "photo.jpg",
      "thumb_url": "/thumb/42",
      "hash": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    }
  ]
}
```
- `content` is an array of content parts.
- `type` can be `"text"` or `"file"` (more types may be added).
  - For `"text"` — `text` field is required.
  - For `"file"` — `file_id` (obtained from upload) is required. `name`, `thumb_url`, `size`, `hash` are **populated by the server** from the file record; any client-supplied values for these are ignored.
- The server verifies that each `file_id` exists (non‑temp), and then increments its reference count.

#### Response (Server ↦ Client)
Success:
```json
{
  "type": "send_message_response",
  "status": "success"
}
```
Error:
```json
{
  "type": "send_message_response",
  "status": "error",
  "message": "Recipient not found"
}
```

---

### 5. Get Messages Chunk

#### Request (Client ↦ Server) — fetch by number
```json
{
  "type": "get_chunk",
  "with": "mmas",
  "chunk": 0
}
```

#### Request (Client ↦ Server) — fetch chunk containing first unread message
```json
{
  "type": "get_chunk",
  "with": "mmas",
  "chunk": "unread"
}
```
- `with` — the other participant's username.
- `chunk` — either a number (0‑based index) or the literal string `"unread"`.
  - When `"unread"` is given, the server calculates the chunk that contains the oldest unread message from `with` to the current user.
- `chunk_size` from config (default 50) determines how many messages per chunk.

#### Response (Server ↦ Client)
```json
{
  "type": "get_messages_response",
  "status": "success",
  "messages": [
    {
      "id": 1,
      "sender": "kot",
      "recipient": "mmas",
      "content": [
        { "type": "text", "text": "1300" },
        {
          "type": "file",
          "file_id": 42,
          "name": "photo.jpg",
          "thumb_url": "/thumb/42",
          "hash": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
          "size": 12345
        }
      ],
      "timestamp": "2025-01-01T12:34:56",
      "read": true
    }
  ],
  "chunk_id": 0
}
```
- `timestamp` — UTC.
- `edited_at` — ISO timestamp when the message was last edited. **Omitted** when the message has never been edited.
- `chunk_id` tells the client which chunk index was actually fetched (useful when requesting `"unread"`).
- On error, the response also carries an optional `message` field.

---

### 6. Get Conversations List

#### Request (Client ↦ Server)
```json
{
  "type": "get_conversations"
}
```

#### Response (Server ↦ Client)
```json
{
  "type": "get_conversations_response",
  "status": "success",
  "conversations": [
    {
      "with": "mmas",
      "last_message": [
        { "type": "text", "text": "1300" }
      ],
      "last_timestamp": "2025-01-01T13:00:00",
      "unread_count": 3,
      "online": true,
      "profile_picture_url": "/profile_pics/42"
    }
  ]
}
```
- `last_message` — the `content` array of the latest message in the conversation (same shape as in `get_messages_response`). Empty array if the conversation has no messages.
- `unread_count` — number of unread messages sent by the other user to the current user.
- `profile_picture_url` — URL of the other user's primary profile picture (`/profile_pics/{file_id}`), or `null`. Key is always present.

---

### 7. Mark Messages as Read

#### Request (Client ↦ Server) — mark a single message
```json
{
  "type": "mark_read",
  "with": "mmas",
  "id": 123
}
```

#### Request (Client ↦ Server) — mark all messages from a sender as read
```json
{
  "type": "mark_read",
  "with": "mmas"
}
```
- If `id` is omitted, all messages from `with` to the current user are marked read.

#### Response (Server ↦ Client)
```json
{
  "type": "mark_read_response",
  "status": "success"
}
```
- On success, read receipts are pushed to the sender(s).

---

### 8. Edit a Message

#### Request (Client ↦ Server)
```json
{
  "type": "edit_message",
  "id": 123,
  "content": [
    { "type": "text", "text": "new corrected text" }
  ]
}
```
- Only the original sender of the message can edit it.
- `content` follows the same format as in `send_message`.
- The server updates the message content and sets `edited_at` to the current time.

#### Response (Server ↦ Client)
Success:
```json
{
  "type": "edit_message_response",
  "status": "success",
  "message": null
}
```
Error:
```json
{
  "type": "edit_message_response",
  "status": "error",
  "message": "Only the sender can edit this message"
}
```
- The `message` key is always present; it is `null` on success.

---

### 9. Delete a Message

#### Request (Client ↦ Server)
```json
{
  "type": "delete_message",
  "id": 123
}
```
- Only the **sender** of the message can delete it.

#### Response (Server ↦ Client)
Success:
```json
{
  "type": "delete_message_response",
  "status": "success"
}
```
Error:
```json
{
  "type": "delete_message_response",
  "status": "error",
  "message": "Only the sender can delete this message"
}
```
- On success, a `message_deleted` push is sent to the other participant.

---

### 10. File Upload Request

#### Request (Client ↦ Server)
```json
{
  "type": "file_upload_request",
  "hash": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
  "name": "document.pdf",
  "size": 12345
}
```
- `hash` — SHA‑256 of the file content (64 hex chars).
- `name` — original file name.
- `size` — file size in bytes (must not exceed `file_max_size_bytes`).

#### Response (Server ↦ Client)

New file (client should upload to the given URL):
```json
{
  "type": "file_upload_response",
  "status": "new",
  "upload_url": "http://server:8080/upload/abc123...",
  "name": "document.pdf"
}
```

Duplicate (file already exists):
```json
{
  "type": "file_upload_response",
  "status": "duplicate",
  "file_id": 42,
  "thumb_url": "/thumb/42",
  "name": "document.pdf"
}
```

Pending (another upload with same hash is in progress):
```json
{
  "type": "file_upload_response",
  "status": "pending",
  "message": "Upload already in progress"
}
```

Error:
```json
{
  "type": "file_upload_response",
  "status": "error",
  "message": "File too large"
}
```

Notes:
- `thumb_url` in the WS `file_upload_response` is **always present** — `null` when the file has no thumbnail, otherwise `/thumb/{file_id}`. It is only populated for `duplicate` responses (where a finalized file record already exists).

---

### 11. Logout

#### Request (Client ↦ Server)
```json
{
  "type": "logout"
}
```
- Invalidates the Bearer token on the server side.
- The WebSocket connection is **not** closed by this request; the client may close it explicitly.

#### Response (Server ↦ Client)
Success:
```json
{
  "type": "logout_response",
  "status": "success"
}
```

Error (if not authenticated):
```json
{
  "type": "error",
  "status": "not_authenticated",
  "message": "Not logged in"
}
```

---

### 12. Pin Message

#### Request (Client ↦ Server)
```json
{
  "type": "pin_message",
  "id": 123
}
```
- Only participants of the conversation can pin the message.

#### Response (Server ↦ Client)
```json
{
  "type": "pin_message_response",
  "status": "success"
}
```
Error:
```json
{
  "type": "pin_message_response",
  "status": "error",
  "message": "Not a participant"
}
```
- On success, a `message_pinned` push is sent to both participants.

---

### 13. Get Pinned Messages

#### Request (Client ↦ Server)
```json
{
  "type": "get_pinned_messages",
  "with": "mmas"
}
```

#### Response (Server ↦ Client)
```json
{
  "type": "get_pinned_messages_response",
  "status": "success",
  "content": [
    {
      "id": 123,
      "chunk_id": 2,
      "content": [
        { "type": "text", "text": "pinned text" }
      ]
    }
  ]
}
```
- `chunk_id` — index of the chunk that contains the pinned message (useful to jump to it).
- Ordered by pin time, ascending (oldest pin first).

---

### 14. Add Profile Picture

#### Request (Client ↦ Server)
```json
{
  "type": "add_profile_picture",
  "hash": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
  "name": "avatar.png",
  "size": 12345
}
```
- Same rules as `file_upload_request` (`hash`, `name`, `size`).

#### Response (Server ↦ Client)
New upload:
```json
{
  "type": "add_profile_picture_response",
  "status": "success",
  "upload_url": "http://server:8080/upload/abc123...",
  "file_id": 42
}
```
Duplicate:
```json
{
  "type": "add_profile_picture_response",
  "status": "duplicate",
  "message": "File already exists"
}
```
Other status values: `payload_too_large`, `invalid_input`, `error`.

- After the HTTP upload completes, the file is automatically registered as a profile picture for the authenticated user.
- The first profile picture added becomes the primary one.
- Fields with no value (`upload_url`, `file_id`, `message`) are omitted, not sent as `null`.

---

### 15. Remove Profile Picture

#### Request (Client ↦ Server)
```json
{
  "type": "remove_profile_picture",
  "id": 7
}
```
- `id` — the profile‑picture row id, **not** the file id.
- Note: `get_profile_pictures_response` does not currently expose this row id. See the note on §16.

#### Response (Server ↦ Client)
```json
{
  "type": "remove_profile_picture_response",
  "status": "success"
}
```
- If the removed picture was primary, the oldest remaining picture is promoted to primary automatically.

---

### 16. Get Profile Pictures

#### Request (Client ↦ Server)
```json
{
  "type": "get_profile_pictures",
  "user_id": 42
}
```
- `user_id` is **required**. To fetch your own, pass your own id.

#### Response (Server ↦ Client)
```json
{
  "type": "get_profile_pictures_response",
  "username": "mmas",
  "status": "success",
  "pictures": [
    {
      "url": "/profile_pics/42",
      "is_primary": true,
      "uploaded_at": "2025-01-01T12:34:56"
    }
  ]
}
```
- `username` — the target user's username. Always present. Empty string on user-not-found errors.
- `url` points to the HTTP endpoint `GET /profile_pics/:file_id`.
- Ordered by upload time, newest first (`is_primary` is **not** used as a sort key).

> **Note:** The per-picture `id` field has been removed from this response. As a result, `remove_profile_picture` and `set_primary_profile_picture` cannot currently be driven from this list alone. If the client needs these operations, a way to surface the row id will need to be reintroduced.

---

### 17. Set Primary Profile Picture

#### Request (Client ↦ Server)
```json
{
  "type": "set_primary_profile_picture",
  "id": 7
}
```
- `id` — the profile‑picture row id (same caveat as §15).

#### Response (Server ↦ Client)
```json
{
  "type": "set_primary_profile_picture_response",
  "status": "success"
}
```

---

## Server Push Messages

### New Message
Pushed to the recipient(s) when a new message is stored.
```json
{
  "type": "new_message",
  "sender": "mmas",
  "content": [
    { "type": "text", "text": "zadaaaan" },
    {
      "type": "file",
      "file_id": 42,
      "name": "photo.jpg",
      "thumb_url": "/thumb/42",
      "hash": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
      "size": 12345
    }
  ],
  "timestamp": "2025-01-01T12:34:56"
}
```

### Message Read Receipt
Pushed to the sender(s) when a message they sent is marked read.
```json
{
  "type": "message_read",
  "message_id": 123,
  "reader": "mmas"
}
```

### Message Edited
Pushed to both participants when a message is edited.
```json
{
  "type": "message_edited",
  "message_id": 123,
  "content": [
    { "type": "text", "text": "new corrected text" }
  ],
  "edited_at": "2025-01-02T10:00:00"
}
```

### Message Deleted
Pushed to the other participant when a message is deleted.
```json
{
  "type": "message_deleted",
  "message_id": 123
}
```

### Message Pinned / Unpinned
Pushed to both participants.
```json
{
  "type": "message_pinned",
  "with": "mmas",
  "content": [
    { "type": "text", "text": "pinned text" }
  ]
}
```
```json
{
  "type": "message_unpinned",
  "with": "mmas",
  "message_id": 123
}
```

### Shutdown Notification
Sent when the server initiates graceful shutdown.
```json
{
  "type": "shutdown",
  "reason": "server shutting down"
}
```

---

## HTTP File Server API
Default addr: `http://server:8080`

All HTTP requests **must** include the header:
```
Authorization: Bearer <token>
```
where `<token>` is the string returned from `signup` or `signin`.

### 1. Upload File

**Endpoint:** `POST /upload/:token`
**Content-Type:** `multipart/form-data`

IMPORTANT: `<token>` ≠ `:token`

- `:token` — the temporary upload token received from `file_upload_request` (status `"new"`).
- The file must be sent as a **single** field (name is not important).
- The server validates:
  - File size ≤ `file_max_size_bytes`
  - MIME type (detected by **content sniffing**, not by extension or the client-supplied `Content-Type` header) is in `allowed_mime_types`
  - Upload token is valid and not expired
  - Request IP matches the IP that originally requested the upload

**Success Response (HTTP 200)**
```json
{
  "type": "file_upload_response",
  "status": "success",
  "upload_url": "http://server:8080/upload/abc123...",
  "file_id": 42,
  "thumb_url": "/thumb/42"
}
```
- `thumb_url` is `null` when no thumbnail was generated (typically non-media files, or media where generation failed).
- All five keys are always present in this HTTP response.

**Error Responses**
- `400 Bad Request` — token expired, filename too long, or invalid file path
- `401 Unauthorized` — request IP does not match the one that created the upload token
- `404 Not Found` — invalid/unknown token
- `413 Payload Too Large`
- `415 Unsupported Media Type`
- `500 Internal Server Error`

### 2. Download File

**Endpoint:** `GET /files/:id`

- `:id` — the numeric `file_id`.
- Returns the original file with appropriate `Content-Type` and `Content-Disposition: inline; filename="..."`.

**Authentication required** (Bearer token).

**Error Responses**
- `401 Unauthorized`
- `404 Not Found` — file missing, temp file, or row absent

### 3. Download Thumbnail

**Endpoint:** `GET /thumb/:id`

- `:id` — numeric `file_id`.
- Returns the thumbnail image. `Content-Type` is derived from the stored thumbnail's extension: `image/png`, `image/webp`, or (fallback) `image/jpeg`.

**Authentication required** (Bearer token).

**Error Responses**
- `401 Unauthorized`
- `404 Not Found` — the file has no `thumb_path`, the thumbnail file is missing on disk, or the file is temp

> **Note:** The server no longer serves bundled placeholder thumbnails. Missing thumbnails return `404`. Client UIs should be prepared to fall back to an icon locally.

### 4. Download Profile Picture

**Endpoint:** `GET /profile_pics/:id`

- `:id` — numeric `file_id`.
- Returns the profile picture with appropriate `Content-Type`.
- Only serves files that are registered in the `profile_pictures` table.

**Authentication required** (Bearer token).

**Error Responses**
- `401 Unauthorized`
- `404 Not Found`

---

## Authentication & Security

- **WebSocket** — authentication is established by the `signup` or `signin` flow. After success, the connection is considered authenticated.
- **HTTP** — every request must carry the Bearer token in the `Authorization` header. Tokens are stored in memory and remain valid until the server restarts.
- **Upload URLs** — one‑time, expire after `file_upload_temp_expiry_secs` (default 300 seconds), and are tied to the specific file hash. They cannot be reused.
- **Upload IP binding** — `POST /upload/:token` is bound to the client IP that originally requested the upload (`requester_ip`). Requests from a different IP are rejected with `401 Unauthorized`.
- **Rate limiting** — when `rate_limit_enabled` is `true`, every WebSocket connection is throttled per client IP using a token bucket (`rate_limit_requests_per_minute`). Exceeding the limit returns:
```json
{
  "type": "error",
  "status": "rate_limited",
  "message": "Too many requests, please slow down"
}
```

---

## Content Moderation

When `model_enabled` is `true`, every `send_message` request has its text parts run through a two-stage filter:

1. **Obfuscation detector** — normalizes and fuzzy-matches Persian profanity patterns.
2. **ONNX classifier** — `model.onnx` + `tokenizer.json` inside `model_dir`.

If the text is flagged, `send_message` is rejected:
```json
{
  "type": "error",
  "status": "content_blocked",
  "message": "Message contains inappropriate content"
}
```

If the classifier fails to run:
```json
{
  "type": "error",
  "status": "model_error",
  "message": "Content filter unavailable"
}
```

Only text parts are inspected; file parts are ignored.

---

## Error Handling

- WebSocket responses always include a `status` field. Except for `success`, the exact error strings are in the `message` field.
- For unknown message types, the server responds with:
```json
{
  "type": "error",
  "status": "unknown_message_type",
  "message": "Unknown message type: <type>"
}
```
- HTTP errors follow standard HTTP status codes with a JSON body:
```json
{
  "type": "error",
  "status": "<snake_case_kind>",
  "message": "<human readable>"
}
```

### Full list of `status` values

| `status` | Meaning |
|---|---|
| `success` | Request succeeded |
| `new` | New file upload token issued |
| `error` | Generic error |
| `duplicate` | File/profile-picture hash already exists |
| `pending` | Upload with same hash already in progress |
| `iccid_error` | ICCID already exists or is invalid |
| `username_error` | Username not found / already exists |
| `password_error` | Incorrect password |
| `invalid_input` | Malformed or out-of-range input |
| `rate_limited` | Per-IP request rate exceeded |
| `unauthorized` | Missing/invalid auth for a protected action |
| `not_authenticated` | Client is not logged in |
| `not_found` | Target resource missing |
| `bad_request` | Malformed or missing field (e.g. missing `user_id`) |
| `payload_too_large` | File exceeds `file_max_size_bytes` |
| `unsupported_media_type` | Detected MIME not in `allowed_mime_types` |
| `content_blocked` | Message rejected by content filter |
| `model_error` | Content filter failed to run |
| `unknown_message_type` | Unrecognized `type` field |

---

## Additional Notes

### Heartbeat (Ping/Pong)

The server sends a WebSocket `Ping` frame every `ping_interval_secs` (default 30 seconds). Clients must respond with a `Pong` frame within `pong_timeout_secs` (default 15 seconds), otherwise the connection will be closed. Most WebSocket libraries handle responding to pings automatically.

### Configurable Limits

The server behavior is controlled by `config.json` (created automatically on first run). The following limits are adjustable:

- `max_username_length`, `max_password_length`, `min_password_length` — length constraints for credentials.
- `search_results_limit` — maximum number of users returned in a search.
- `chunk_size` — number of messages per chunk (used by `get_chunk`).
- `file_max_size_bytes` — maximum allowed file upload size.
- `allowed_mime_types` — list of accepted MIME types for file uploads.
- `file_upload_temp_expiry_secs` — lifetime of temporary upload tokens.
- `cleanup_interval_secs` — how often the cleanup task runs.
- `db_pool_max_size` — database connection pool size.

Additional keys present in `config.json`:

| Key | Purpose |
|---|---|
| `server_addr`, `server_port`, `http_port` | Bind address and ports |
| `shutdown_timeout_secs` | Grace period for graceful shutdown |
| `db_path`, `db_journal_mode`, `db_synchronous` | SQLite tuning |
| `iccid_pepper` | Pepper mixed into ICCID hash |
| `allowed_ccs` | Allowed ICCID country codes (after `89`) |
| `bcrypt_cost` | bcrypt cost factor (4–31) |
| `max_iccid_length` | Max ICCID length accepted |
| `ping_interval_secs`, `pong_timeout_secs` | WebSocket heartbeat timings |
| `rate_limit_enabled`, `rate_limit_requests_per_minute`, `rate_limit_cleanup_interval_secs` | Rate limiter |
| `file_storage_dir`, `temp_upload_dir` | Filesystem locations |
| `max_filename_length` | Max original filename length |
| `file_rename_sync_delay_ms` | Post-rename sync delay |
| `thumbnail_scale_percent`, `thumbnail_format`, `thumbnail_quality`, `thumbnail_suffix` | Thumbnail generation |
| `video_thumbnail_seconds` | Seek time for video thumbnails |
| `ffmpeg_path` | Path to ffmpeg binary |
| `pdf_thumbnail_dpi`, `pdftoppm_path` | PDF thumbnail generation |
| `font_thumbnail_text`, `font_thumbnail_bg`, `font_thumbnail_fg` | Font thumbnail rendering |
| `thumb_url_path` | HTTP prefix returned for thumbnails |
| `hash_subdir_depth` | Subdirectory sharding depth for stored files |
| `temp_token_bytes` | Entropy of temporary upload tokens |
| `upload_url_scheme` | `http` or `https` in generated upload URLs |
| `model_enabled`, `model_dir` | Content moderation model |
| `debug` | Verbose logging |

### File Content Parts (`"type": "file"`)

Every file content part emitted by the server carries the following fields where available:

| Field | Type | Source |
|---|---|---|
| `file_id` | integer | From the message content |
| `name` | string | `file_metadata.original_name` |
| `thumb_url` | string | `/thumb/{file_id}` — only if a thumbnail exists |
| `size` | integer | `file_metadata.size` in bytes |
| `hash` | string | `file_metadata.hash` (SHA‑256 hex) |

All are optional in the JSON. `hash` is populated server-side from the file record; a client-supplied `hash` is ignored on inbound `send_message`/`edit_message` and replaced with the DB value before storage and before pushing.

### Binary Messages

The server currently only handles text WebSocket frames (`Message::Text`). Binary frames are ignored and logged as a warning. Clients should only send UTF‑8 text messages.

### Cleanup Internals

The server runs a background task every `cleanup_interval_secs` (default 1 hour) that:

- Deletes temporary upload files whose token has expired.
- Removes finalized files that are no longer referenced in any direct message (orphaned files).

### Profile Pictures Push

There are **no** WebSocket push events for profile-picture changes. Clients re-fetch via `get_profile_pictures`, `search_user`, or `get_conversations` when they need an updated picture URL.
