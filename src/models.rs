use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DetectionMessage {
    pub image: String,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_message_serialize_roundtrip() {
        let msg = DetectionMessage {
            image: "base64data".into(),
            message: "Seal detected!".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: DetectionMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.image, "base64data");
        assert_eq!(parsed.message, "Seal detected!");
    }

    #[test]
    fn detection_message_deserialize_from_json() {
        let json = r#"{"image":"abc123","message":"hello"}"#;
        let msg: DetectionMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.image, "abc123");
        assert_eq!(msg.message, "hello");
    }

    #[test]
    fn detection_message_empty_fields() {
        let msg = DetectionMessage {
            image: "".into(),
            message: "".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: DetectionMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.image, "");
        assert_eq!(parsed.message, "");
    }

    #[test]
    fn detection_message_all_fields_in_json() {
        let msg = DetectionMessage {
            image: "img".into(),
            message: "msg".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(v.get("image").is_some());
        assert!(v.get("message").is_some());
    }

    #[test]
    fn detection_message_missing_field_fails() {
        let json = r#"{"image":"abc"}"#;
        let result = serde_json::from_str::<DetectionMessage>(json);
        assert!(result.is_err());
    }

    #[test]
    fn detection_message_extra_fields_ignored() {
        let json = r#"{"image":"abc","message":"msg","extra":"field"}"#;
        let msg: DetectionMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.image, "abc");
    }

    #[test]
    fn detection_message_clone() {
        let msg = DetectionMessage {
            image: "data".into(),
            message: "text".into(),
        };
        let cloned = msg.clone();
        assert_eq!(cloned.image, msg.image);
        assert_eq!(cloned.message, msg.message);
    }

    #[test]
    fn detection_message_with_large_image() {
        let large_image = "A".repeat(1_000_000);
        let msg = DetectionMessage {
            image: large_image.clone(),
            message: "test".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: DetectionMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.image.len(), 1_000_000);
    }
}
