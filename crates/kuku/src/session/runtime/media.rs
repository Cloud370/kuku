use sha2::{Digest, Sha256};

use crate::execution::{ImageMime, ModelImage, OutputReference, MAX_MODEL_IMAGE_BYTES};
use crate::tool::builtin::run_command::output::{
    framed_bytes, read_complete_stdout_bytes, OutputLimits, OutputWriter, MAX_FRAME_BYTES,
};
use crate::{Error, Result};

use super::record::Record;
use super::retention::{open_retained_output, output_dir};
use super::{ensure_open, OutputStream, Session};

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
        let id = image_id(agent, bytes);
        let image = ModelImage {
            mime,
            output: OutputReference {
                id: id.clone(),
                offset: 0,
                bytes: bytes.len() as u64,
            },
        };
        let record = Record::ModelImageSaved {
            agent: agent.into(),
            turn,
            image: image.clone(),
        };
        {
            let mut state = self.inner.state.lock().unwrap();
            ensure_open(&state)?;
            if let Some(retained) = state.model_images.get(&id) {
                if retained.image == image && state.outputs.owned_by(agent, &id) {
                    if retained.turn != turn {
                        self.commit(&mut state, record)?;
                    }
                    return Ok(image);
                }
            }
        }
        let mut reservation =
            self.reserve_retained_output(agent, &id, Some(framed_bytes(image.output.bytes)), None)?;
        let mut writer = OutputWriter::create(
            &output_dir(self.events_path())?,
            &id,
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

        let mut state = self.inner.state.lock().unwrap();
        ensure_open(&state)?;
        self.commit(&mut state, record)?;
        reservation.keep();
        Ok(image)
    }

    pub(crate) fn read_model_image(&self, agent: &str, image: &ModelImage) -> Result<Vec<u8>> {
        let reference = &image.output;
        {
            let state = self.inner.state.lock().unwrap();
            if state
                .model_images
                .get(&reference.id)
                .map(|retained| &retained.image)
                != Some(image)
                || !state.outputs.owned_by(agent, &reference.id)
            {
                return Err(Error::OutputUnavailable(reference.id.clone()));
            }
        }
        if reference.offset != 0 || reference.bytes > MAX_MODEL_IMAGE_BYTES as u64 {
            return Err(Error::InvalidEventStream(
                "invalid model image extent".into(),
            ));
        }
        let file = open_retained_output(&output_dir(self.events_path())?, &reference.id)?;
        let bytes = read_complete_stdout_bytes(file, reference.bytes as usize)?;
        if ImageMime::detect(&bytes) != Some(image.mime) || image_id(agent, &bytes) != reference.id
        {
            return Err(Error::InvalidEventStream(
                "model image content changed".into(),
            ));
        }
        Ok(bytes)
    }
}

fn image_id(agent: &str, bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(agent.as_bytes());
    hash.update([0]);
    hash.update(bytes);
    format!("image_{:x}", hash.finalize())
}
