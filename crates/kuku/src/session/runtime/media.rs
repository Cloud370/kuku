use sha2::{Digest, Sha256};

use crate::execution::{
    ImageMime, InputImage, MAX_INPUT_IMAGES, MAX_MODEL_IMAGE_BYTES, ModelImage, OutputReference,
};
use crate::tool::builtin::run_command::output::{
    MAX_FRAME_BYTES, OutputLimits, OutputWriter, framed_bytes, read_complete_stdout_bytes,
};
use crate::{Error, Result};

use super::record::Record;
use super::retention::{open_retained_output, output_dir};
use super::{OutputStream, Session, ensure_open};

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(super) struct RetainedModelImage {
    pub(super) agent: String,
    pub(super) turn: Option<u64>,
    pub(super) digest: String,
    pub(super) image: ModelImage,
    #[serde(default)]
    pub(super) referenced: bool,
}

impl Session {
    /// Save an encoded image for a later input to this Agent.
    pub async fn save_input_image(&self, agent: &str, bytes: &[u8]) -> Result<InputImage> {
        let image = InputImage::from_bytes(bytes)?;
        self.retain_image(agent, None, image.key(agent), image.mime, bytes)
            .await?;
        Ok(image)
    }

    pub(super) fn validate_input_images(
        &self,
        state: &super::record::State,
        input: &super::Input,
        config: &super::AgentConfig,
    ) -> Result<()> {
        if input.images.is_empty() {
            return Ok(());
        }
        if input.images.len() > MAX_INPUT_IMAGES {
            return Err(Error::InvalidArgument(format!(
                "attach up to {MAX_INPUT_IMAGES} images"
            )));
        }
        self.validate_image_route(config)?;
        for image in &input.images {
            input_image_output(state, &input.target, image)?;
        }
        Ok(())
    }

    pub(super) fn validate_image_route(&self, config: &super::AgentConfig) -> Result<()> {
        if self.inner.options.model_adapter.is_some()
            || self.resolve_model(config)?.capabilities.media_adapter
                != crate::config::MediaAdapterSupport::ResponsesImageToolResults
        {
            return Err(Error::InvalidArgument(
                "The selected model cannot receive images. Switch to a model with image input or remove the attachments.".into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn read_input_image(&self, agent: &str, image: &InputImage) -> Result<Vec<u8>> {
        let output = input_image_output(&self.inner.state.lock().unwrap(), agent, image)?;
        self.read_model_image(agent, &output)
    }

    pub(crate) async fn retain_model_image(
        &self,
        agent: &str,
        turn: u64,
        mime: ImageMime,
        bytes: &[u8],
    ) -> Result<ModelImage> {
        self.retain_image(agent, Some(turn), image_digest(agent, bytes), mime, bytes)
            .await
    }

    async fn retain_image(
        &self,
        agent: &str,
        turn: Option<u64>,
        digest: String,
        mime: ImageMime,
        bytes: &[u8],
    ) -> Result<ModelImage> {
        if bytes.len() > MAX_MODEL_IMAGE_BYTES || ImageMime::detect(bytes) != Some(mime) {
            return Err(Error::InvalidArgument("invalid or oversized image".into()));
        }
        let _write = self.inner.value_writes.lock().await;
        {
            let mut state = self.inner.state.lock().unwrap();
            ensure_open(&state)?;
            if let Some(id) = state.model_image_ids.get(&digest) {
                if state.outputs.owned_by(agent, id) {
                    let retained = &state.model_images[id];
                    let image = retained.image.clone();
                    if retained.turn != turn {
                        self.commit(
                            &mut state,
                            Record::ModelImageSaved {
                                agent: agent.into(),
                                turn,
                                digest,
                                image: image.clone(),
                            },
                        )?;
                    }
                    return Ok(image);
                }
            }
        }
        let mut reservation =
            self.reserve_retained_output(agent, Some(framed_bytes(bytes.len() as u64)), None)?;
        let mut writer = OutputWriter::create(
            &output_dir(self.events_path())?,
            reservation.id(),
            OutputLimits {
                max_output_bytes: reservation.max_bytes(),
                max_preview_bytes: 0,
            },
        )
        .await?;
        for chunk in bytes.chunks(MAX_FRAME_BYTES) {
            writer.write(OutputStream::Stdout, chunk).await?;
        }
        writer.finish(false).await?;

        let image = ModelImage {
            mime,
            output: OutputReference {
                id: reservation.id().into(),
                offset: 0,
                bytes: bytes.len() as u64,
            },
        };
        let mut state = self.inner.state.lock().unwrap();
        ensure_open(&state)?;
        self.commit(
            &mut state,
            Record::ModelImageSaved {
                agent: agent.into(),
                turn,
                digest,
                image: image.clone(),
            },
        )?;
        reservation.keep();
        Ok(image)
    }

    pub(crate) fn read_model_image(&self, agent: &str, image: &ModelImage) -> Result<Vec<u8>> {
        let reference = &image.output;
        let (digest, input) = {
            let state = self.inner.state.lock().unwrap();
            let retained = state
                .model_images
                .get(&reference.id)
                .filter(|retained| retained.image == *image)
                .filter(|_| state.outputs.owned_by(agent, &reference.id))
                .ok_or_else(|| Error::OutputUnavailable(reference.id.clone()))?;
            (retained.digest.clone(), retained.turn.is_none())
        };
        if reference.offset != 0 || reference.bytes > MAX_MODEL_IMAGE_BYTES as u64 {
            return Err(Error::InvalidEventStream(
                "invalid model image extent".into(),
            ));
        }
        let file = open_retained_output(&output_dir(self.events_path())?, &reference.id)?;
        let bytes = read_complete_stdout_bytes(file, reference.bytes as usize)?;
        let actual = if input {
            InputImage::from_bytes(&bytes)?.key(agent)
        } else {
            image_digest(agent, &bytes)
        };
        if ImageMime::detect(&bytes) != Some(image.mime) || actual != digest {
            return Err(Error::InvalidEventStream(
                "model image content changed".into(),
            ));
        }
        Ok(bytes)
    }
}

fn input_image_output(
    state: &super::record::State,
    agent: &str,
    image: &InputImage,
) -> Result<ModelImage> {
    let key = image.key(agent);
    let output = state
        .model_image_ids
        .get(&key)
        .filter(|id| state.outputs.owned_by(agent, id))
        .and_then(|id| state.model_images.get(id))
        .filter(|retained| {
            retained.image.mime == image.mime && retained.image.output.bytes == image.bytes as u64
        })
        .ok_or_else(|| Error::InvalidArgument("image is not saved for the recipient".into()))?;
    Ok(output.image.clone())
}

fn image_digest(agent: &str, bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(agent.as_bytes());
    hash.update([0]);
    hash.update(bytes);
    format!("{:x}", hash.finalize())
}
