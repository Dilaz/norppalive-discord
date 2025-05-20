use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DetectionMessage {
    pub image: String,
    pub message: String,
} 