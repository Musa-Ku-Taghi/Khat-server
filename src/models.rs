use std::fmt;

use serde::de::{Error, Visitor};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub enum ChunkSpec {
    Number(u64),
    Unread,
}

impl<'de> Deserialize<'de> for ChunkSpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ChunkSpecVisitor;

        impl<'de> Visitor<'de> for ChunkSpecVisitor {
            type Value = ChunkSpec;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a number or the string \"unread\"")
            }

            fn visit_u64<E: Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(ChunkSpec::Number(v))
            }

            fn visit_str<E: Error>(self, v: &str) -> Result<Self::Value, E> {
                if v == "unread" {
                    Ok(ChunkSpec::Unread)
                } else {
                    Err(E::custom(format!("expected \"unread\", got {v}")))
                }
            }
        }

        deserializer.deserialize_any(ChunkSpecVisitor)
    }
}

#[derive(Debug, Deserialize)]
pub struct RequestPayload {
    #[serde(rename = "type")]
    pub msg_type: String,
    #[serde(default)]
    pub iccid: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub recipient: Option<String>,
    #[serde(default)]
    pub content: Option<Vec<ContentPart>>,
    #[serde(default)]
    pub with: Option<String>,
    #[serde(default)]
    pub chunk: Option<ChunkSpec>,
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default)]
    pub hash: Option<String>,
    #[serde(default)]
    pub hashes: Option<Vec<String>>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Target {
    pub message_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunk_id: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ContentPart {
    pub r#type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumb_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark_down: Option<Vec<(u64, u64, String)>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<Target>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Message {
    pub id: i64,
    pub sender: String,
    pub recipient: String,
    pub content: Vec<ContentPart>,
    pub timestamp: String,
    pub read: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edited_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SearchUserResponse {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub status: ResponseStatus,
    pub results: Vec<UserSearchResult>,
}

#[derive(Debug, Serialize)]
pub struct UserSearchResult {
    pub username: String,
    pub online: bool,
    pub profile_picture_url: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SignupResponse {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub status: ResponseStatus,
    pub token: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct LoginResponse {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub status: ResponseStatus,
    pub token: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Conversation {
    #[serde(rename = "with")]
    pub with_user: String,
    pub last_message: Vec<ContentPart>,
    pub last_timestamp: String,
    pub unread_count: i64,
    pub online: bool,
    pub profile_picture_url: Option<String>,
    pub locked: bool,
}

#[derive(Debug, Serialize)]
pub struct SendMessageResponse {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub status: ResponseStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct GetMessagesResponse {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub status: ResponseStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub messages: Option<Vec<Message>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk_id: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct GetConversationsResponse {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub status: ResponseStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversations: Option<Vec<Conversation>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MarkReadResponse {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub status: ResponseStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct NewMessagePush {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub sender: String,
    pub content: Vec<ContentPart>,
    pub timestamp: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct MessagePinnedPush {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub with: String,
    pub content: Vec<ContentPart>,
}

#[derive(Debug, Serialize, Clone)]
pub struct MessageReadPush {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub message_id: i64,
    pub reader: String,
}

#[derive(Debug, Serialize)]
pub struct AddConversationLockResponse {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub status: ResponseStatus,
    pub upload_urls: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct OpenConversationLockResponse {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub status: ResponseStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upload_url: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct FaceDetectionResponse {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct FileUploadResponse {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub status: ResponseStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upload_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_id: Option<i64>,
    pub thumb_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DeleteMessageResponse {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub status: ResponseStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PinMessageResponse {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub status: ResponseStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct EditMessageResponse {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub status: ResponseStatus,
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct GetPinnedMessagesResponse {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub status: ResponseStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<PinnedMessage>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PinnedMessage {
    pub id: i64,
    pub chunk_id: u64,
    pub content: Vec<ContentPart>,
}

#[derive(Debug, Serialize)]
pub struct ProfilePictureAddResponse {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub status: ResponseStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upload_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ProfilePictureRemoveResponse {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub status: ResponseStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ProfilePictureGetResponse {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub username: String,
    pub status: ResponseStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pictures: Option<Vec<ProfilePictureInfo>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ProfilePictureInfo {
    pub url: String,
    pub is_primary: bool,
    pub uploaded_at: String,
}

#[derive(Debug, Serialize)]
pub struct ProfilePictureSetPrimaryResponse {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub status: ResponseStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ContentSettingsResponse {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub spam: bool,
    pub obscene: bool,
    pub hate: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseStatus {
    Success,
    New,
    Error,
    Duplicate,
    Pending,
    IccidError,
    UsernameError,
    PasswordError,
    InvalidInput,
    RateLimited,
    Unauthorized,
    NotFound,
    BadRequest,
    PayloadTooLarge,
    UnsupportedMediaType,
    ContentBlocked,
    ModelError,
    UnknownMessageType,
    NotAuthenticated,
    Ready,
    ChatNotLocked,
    LockedChatVerificationRequired,
}

impl fmt::Display for ResponseStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            ResponseStatus::Success => "success",
            ResponseStatus::New => "new",
            ResponseStatus::Error => "error",
            ResponseStatus::Duplicate => "duplicate",
            ResponseStatus::Pending => "pending",
            ResponseStatus::IccidError => "iccid_error",
            ResponseStatus::UsernameError => "username_error",
            ResponseStatus::PasswordError => "password_error",
            ResponseStatus::InvalidInput => "invalid_input",
            ResponseStatus::RateLimited => "rate_limited",
            ResponseStatus::Unauthorized => "unauthorized",
            ResponseStatus::NotFound => "not_found",
            ResponseStatus::BadRequest => "bad_request",
            ResponseStatus::PayloadTooLarge => "payload_too_large",
            ResponseStatus::UnsupportedMediaType => "unsupported_media_type",
            ResponseStatus::ContentBlocked => "content_blocked",
            ResponseStatus::ModelError => "model_error",
            ResponseStatus::UnknownMessageType => "unknown_message_type",
            ResponseStatus::NotAuthenticated => "not_authenticated",
            ResponseStatus::Ready => "ready",
            ResponseStatus::ChatNotLocked => "chat_not_locked",
            ResponseStatus::LockedChatVerificationRequired => "locked_chat_verification_required",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UploadType {
    Message,
    Profile,
    FaceLockRef,
    FaceLockProbe,
}

impl fmt::Display for UploadType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UploadType::Message => f.write_str("message"),
            UploadType::Profile => f.write_str("profile"),
            UploadType::FaceLockRef => f.write_str("face_lock_ref"),
            UploadType::FaceLockProbe => f.write_str("face_lock_probe"),
        }
    }
}
