use sha2::{Digest, Sha256};

use crate::execution::{ImageMime, MAX_MODEL_IMAGE_BYTES, ModelImage, OutputReference};
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
    pub(super) turn: u64,
    pub(super) digest: String,
    pub(super) image: ModelImage,
}

impl Session {
    pub(crate) async fn retain_model_image(
        &self,
        agent: &str,
        turn: u64,
        mime: ImageMime,
        bytes: &[u8],
    ) -> Result<ModelImage> {
        let _write = self.inner.value_writes.lock().await;
        if bytes.len() > MAX_MODEL_IMAGE_BYTES || ImageMime::detect(bytes) != Some(mime) {
            return Err(Error::InvalidArgument("invalid or oversized image".into()));
        }
        let digest = image_digest(agent, bytes);
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
        let digest = {
            let state = self.inner.state.lock().unwrap();
            let retained = state
                .model_images
                .get(&reference.id)
                .filter(|retained| retained.image == *image)
                .filter(|_| state.outputs.owned_by(agent, &reference.id))
                .ok_or_else(|| Error::OutputUnavailable(reference.id.clone()))?;
            retained.digest.clone()
        };
        if reference.offset != 0 || reference.bytes > MAX_MODEL_IMAGE_BYTES as u64 {
            return Err(Error::InvalidEventStream(
                "invalid model image extent".into(),
            ));
        }
        let file = open_retained_output(&output_dir(self.events_path())?, &reference.id)?;
        let bytes = read_complete_stdout_bytes(file, reference.bytes as usize)?;
        if ImageMime::detect(&bytes) != Some(image.mime) || image_digest(agent, &bytes) != digest {
            return Err(Error::InvalidEventStream(
                "model image content changed".into(),
            ));
        }
        Ok(bytes)
    }
}

fn image_digest(agent: &str, bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(agent.as_bytes());
    hash.update([0]);
    hash.update(bytes);
    format!("{:x}", hash.finalize())
}
