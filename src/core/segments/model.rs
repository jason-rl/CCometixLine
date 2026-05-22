use super::{Segment, SegmentData};
use crate::config::{InputData, ModelConfig, SegmentId};
use std::collections::HashMap;

#[derive(Default)]
pub struct ModelSegment;

impl ModelSegment {
    pub fn new() -> Self {
        Self
    }
}

impl Segment for ModelSegment {
    fn collect(&self, input: &InputData) -> Option<SegmentData> {
        let model_config = ModelConfig::load();
        let mut metadata = HashMap::new();
        metadata.insert("model_id".to_string(), input.model.id.clone());
        metadata.insert("display_name".to_string(), input.model.display_name.clone());

        let primary = if let Some(display) = model_config.get_model_display(&input.model.id) {
            metadata.insert("model_glyph".to_string(), display.glyph);
            metadata.insert("model_rest".to_string(), display.rest);
            metadata.insert("model_family".to_string(), display.family);
            display.plain
        } else if let Some(config_name) = model_config.get_display_name(&input.model.id) {
            config_name
        } else {
            let base = if input.model.display_name.is_empty() {
                input.model.id.clone()
            } else {
                input.model.display_name.clone()
            };
            match model_config.get_display_suffix(&input.model.id) {
                Some(suffix) => format!("{}{}", base, suffix),
                None => base,
            }
        };

        Some(SegmentData {
            primary,
            secondary: String::new(),
            metadata,
        })
    }

    fn id(&self) -> SegmentId {
        SegmentId::Model
    }
}
