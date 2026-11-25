use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use kuku::config::ImageInputSupport;
use kuku_client::{ImageAttachment, ImageMarker, MAX_INPUT_IMAGES, Timing};

use crate::clipboard::{ClipboardImage, PasteSource, Pasted};
use crate::jobs::{Job, Selection};
use crate::menu::{Choice, ChoiceKind, PickerKind};

use super::send_route::RouteChange;
use super::{Address, App, Modal, Pane};

const DEFAULT_SEND_DELAY: Duration = Duration::from_secs(3);

#[derive(Debug)]
pub(crate) enum ImageAutoSend {
    AwaitingRender,
    Counting(Instant),
}

impl ImageAutoSend {
    pub(crate) fn begin(&mut self, now: Instant) {
        if matches!(self, Self::AwaitingRender) {
            *self = Self::Counting(now + DEFAULT_SEND_DELAY);
        }
    }

    pub(crate) fn seconds(&self, now: Instant) -> Option<u64> {
        let Self::Counting(at) = self else {
            return None;
        };
        Some(at.saturating_duration_since(now).as_millis().div_ceil(1000) as u64)
    }
}

#[derive(Clone, Debug)]
pub(crate) enum DraftImage {
    Clipboard {
        label: String,
        source: Arc<ClipboardImage>,
        transparent: bool,
    },
    Recovered(ImageAttachment),
}

impl DraftImage {
    fn label(&self) -> &str {
        match self {
            Self::Clipboard { label, .. } => label,
            Self::Recovered(image) => &image.marker.label,
        }
    }

    fn transparent(&self) -> bool {
        match self {
            Self::Clipboard { transparent, .. } => *transparent,
            Self::Recovered(image) => image.transparent,
        }
    }

    fn rename(&mut self, label: String) {
        match self {
            Self::Clipboard { label: current, .. } => *current = label,
            Self::Recovered(image) => image.marker.label = label,
        }
    }
}

pub(crate) struct PrepareImages {
    pub(crate) ticket: u64,
    pub(crate) entries: Vec<(usize, DraftImage)>,
    pub(crate) original: bool,
}

impl PrepareImages {
    pub(crate) fn encode(self) -> Result<Vec<ImageAttachment>, String> {
        self.entries
            .into_iter()
            .map(|(offset, entry)| match entry {
                DraftImage::Recovered(mut image) => {
                    image.marker.offset = offset;
                    if !self.original {
                        let bytes = base64::engine::general_purpose::STANDARD
                            .decode(image.data)
                            .map_err(|error| error.to_string())?;
                        let source = crate::clipboard::decode_image(bytes)?;
                        image.data = source.encode(false)?;
                        image.transparent = source.transparent;
                    }
                    Ok(image)
                }
                DraftImage::Clipboard {
                    label,
                    source,
                    transparent,
                } => {
                    let data = source.encode(self.original)?;
                    Ok(ImageAttachment {
                        marker: ImageMarker { offset, label },
                        data,
                        transparent,
                    })
                }
            })
            .collect()
    }
}

#[derive(Debug)]
pub(super) struct ImageSend {
    ticket: u64,
    revision: u64,
    timing: Timing,
    route: Option<RouteChange>,
}

impl App {
    pub(super) fn recalled_image_draft(
        &mut self,
        selection: Selection,
        event_id: u64,
        result: Result<kuku_client::MessageDraft, kuku_client::Error>,
    ) {
        if selection.target.workspace != self.workspace {
            return;
        }
        let address = Address::from_target(&selection.target);
        let Some(pane) = self.panes.get_mut(&address) else {
            return;
        };
        if pane.draft.recalled_image_event() != Some(event_id) {
            return;
        }
        match result {
            Ok(draft) => {
                pane.draft.load_recalled_images(event_id, &draft);
                pane.draft_images = draft
                    .images
                    .into_iter()
                    .map(DraftImage::Recovered)
                    .collect();
                pane.draft_changed();
            }
            Err(error) => {
                pane.cancel_image_recall();
                if address == self.active {
                    self.problem(format!("Could not load recalled images · {error}"));
                }
            }
        }
    }

    pub(crate) fn paste_transfer(&mut self, text: String) {
        if self.modal.is_some() {
            self.paste(&text);
        } else if text.is_empty() {
            self.begin_clipboard_read(PasteSource::Clipboard { image_only: true });
        } else if crate::clipboard::image_path_candidate(&text).is_some() {
            self.begin_clipboard_read(PasteSource::ImagePath(text));
        } else {
            self.paste(&text);
        }
    }

    pub(crate) fn paste_clipboard(&mut self) {
        self.begin_clipboard_read(PasteSource::Clipboard { image_only: false });
    }

    fn begin_clipboard_read(&mut self, source: PasteSource) {
        if self.pane().clipboard_ticket.is_some() {
            return;
        }
        let ticket = self.modal_id();
        self.pane_mut().clipboard_ticket = Some(ticket);
        self.effects
            .push(Job::PasteClipboard(self.selection(), ticket, source));
    }

    pub(super) fn clipboard_pasted(
        &mut self,
        selection: Selection,
        ticket: u64,
        pasted: Result<Pasted, String>,
    ) {
        if selection.target.workspace != self.workspace {
            return;
        }
        let address = Address::from_target(&selection.target);
        let active = address == self.active;
        let Some(pane) = self.panes.get_mut(&address) else {
            return;
        };
        if pane.clipboard_ticket != Some(ticket) {
            return;
        }
        pane.clipboard_ticket = None;
        match pasted {
            Ok(Pasted::Text(text)) if !text.is_empty() => {
                if active && self.modal.is_none() {
                    self.paste(&text);
                } else {
                    pane.draft.insert(&text);
                    pane.draft_changed();
                }
            }
            Ok(Pasted::Image(source)) if pane.attached_images().count() < MAX_INPUT_IMAGES => {
                let label = pane.next_image_label();
                let transparent = source.transparent;
                let preceding = &pane.draft.text()[..pane.draft.cursor()];
                if !preceding.is_empty() && !preceding.ends_with(char::is_whitespace) {
                    pane.draft.insert(" ");
                }
                pane.draft.insert_atom(&label);
                pane.draft.insert(" ");
                pane.draft_images.push(DraftImage::Clipboard {
                    label,
                    source: Arc::new(source),
                    transparent,
                });
                pane.draft_changed();
                if active && self.modal.is_none() {
                    self.focus_composer();
                }
            }
            Ok(Pasted::Image(_)) if active => {
                self.problem(format!("Attach up to {MAX_INPUT_IMAGES} images"));
            }
            Err(error) if active => self.problem(format!("Clipboard · {error}")),
            _ => {}
        }
    }

    fn open_image_options(&mut self, timing: Timing, route: Option<&RouteChange>) {
        let quality = |original| ChoiceKind::SendImages {
            original,
            timing,
            route: route.cloned().map(Box::new),
        };
        let picker = self.picker(
            PickerKind::Images,
            vec![
                Choice::new(
                    "Compressed",
                    "JPEG · PNG for transparent images",
                    quality(false),
                ),
                Choice::new(
                    "Original",
                    "Keep the source pixels and dimensions",
                    quality(true),
                ),
            ],
        );
        picker.image_auto_send = Some(ImageAutoSend::AwaitingRender);
    }

    pub(crate) fn cancel_image_countdown(&mut self) {
        if let Some(Modal::Picker(picker)) = &mut self.modal {
            picker.image_auto_send = None;
        }
    }

    pub(super) fn image_countdown_deadline(&self, now: Instant) -> Option<Instant> {
        let Some(Modal::Picker(picker)) = &self.modal else {
            return None;
        };
        let Some(ImageAutoSend::Counting(at)) = picker.image_auto_send else {
            return None;
        };
        [at - Duration::from_secs(2), at - Duration::from_secs(1)]
            .into_iter()
            .find(|tick| *tick > now)
            .or(Some(at))
    }

    pub(crate) fn tick_image_countdown(&mut self, now: Instant) -> bool {
        let Some(Modal::Picker(picker)) = &self.modal else {
            return false;
        };
        let Some(ImageAutoSend::Counting(at)) = picker.image_auto_send else {
            return false;
        };
        if at <= now
            && let Some(
                choice @ ChoiceKind::SendImages {
                    original: false, ..
                },
            ) = picker.selected_choice()
        {
            self.choose(PickerKind::Images, choice, picker.ticket.id);
        }
        true
    }

    pub(crate) fn encode_draft_images(
        &mut self,
        original: bool,
        timing: Timing,
        route: Option<RouteChange>,
    ) {
        let pane = self.pane();
        let revision = pane.draft_revision;
        let entries: Vec<_> = pane
            .draft_images
            .iter()
            .filter_map(|entry| {
                let range = pane.draft.atom_range(entry.label())?;
                Some((range.start, entry.clone()))
            })
            .collect();
        if entries.is_empty() {
            return;
        }
        let ticket = self.modal_id();
        self.pane_mut().image_send = Some(ImageSend {
            ticket,
            revision,
            timing,
            route,
        });
        self.effects.push(Job::EncodeImages(
            self.selection(),
            PrepareImages {
                ticket,
                entries,
                original,
            },
        ));
    }

    pub(super) fn prepare_draft_images(
        &mut self,
        timing: Timing,
        route: Option<&RouteChange>,
    ) -> bool {
        let pane = self.pane();
        if pane.clipboard_ticket.is_some()
            || pane.image_send.is_some()
            || pane.draft.recalled_image_event().is_some()
        {
            return true;
        }
        if !pane.encoded_images.is_empty() || pane.attached_images().next().is_none() {
            return false;
        }
        let image_input = if let Some(snapshot) = &pane.snapshot {
            snapshot
                .pending_selection
                .as_ref()
                .or(snapshot.selection.as_ref())
                .map(|selection| selection.image_input)
        } else {
            pane.initial_model
                .as_ref()
                .map(|model| model.capabilities.image_input_support(true))
        };
        let problem = match image_input {
            Some(ImageInputSupport::UnsupportedByModel) => {
                Some("This model does not accept images · switch models or remove image markers")
            }
            Some(ImageInputSupport::Unconfirmed) => Some(
                "Image input is unverified · test and save in Model setup or remove image markers",
            ),
            Some(ImageInputSupport::UnsupportedOnRoute) => Some(
                "This model supports images, but this connection cannot send them · switch routes or remove image markers",
            ),
            Some(ImageInputSupport::Supported) | None => None,
        };
        if let Some(problem) = problem {
            self.problem(problem);
            return true;
        }
        self.open_image_options(timing, route);
        true
    }

    pub(super) fn images_encoded(
        &mut self,
        selection: Selection,
        ticket: u64,
        result: Result<Vec<ImageAttachment>, String>,
    ) {
        let address = Address::from_target(&selection.target);
        let active = selection == self.selection();
        let Some(pane) = self.panes.get_mut(&address) else {
            return;
        };
        if !pane
            .image_send
            .as_ref()
            .is_some_and(|send| send.ticket == ticket)
        {
            return;
        }
        let send = pane.image_send.take().expect("matching image preparation");
        if !active || send.revision != pane.draft_revision || self.modal.is_some() {
            return;
        }
        match result {
            Ok(images) => {
                self.pane_mut().encoded_images = images;
                self.send_with_route(send.timing, send.route.as_ref());
                self.pane_mut().encoded_images.clear();
            }
            Err(error) => self.problem(format!("{error} · draft kept")),
        }
    }
}

impl Pane {
    pub(super) fn remember_history_entry(&mut self, entry: &kuku_client::HistoryEntry) {
        let Some(text) = &entry.text else { return };
        self.cancel_image_recall();
        if entry
            .details
            .get("has_images")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        {
            self.draft.remember_image_event(text, entry.id);
        } else {
            self.draft.remember(text);
        }
    }

    pub(super) fn cancel_image_recall(&mut self) {
        if self.draft.recalled_image_event().is_some() {
            self.draft.cancel_recall();
            self.draft_images = self.recall_images.take().unwrap_or_default();
            self.draft_changed();
        }
    }

    pub(crate) fn recall_draft(&mut self, direction: isize) {
        self.draft.recall(direction);
        if let Some(images) = self.draft.recalled_images() {
            self.recall_images
                .get_or_insert_with(|| std::mem::take(&mut self.draft_images));
            self.draft_images = images.iter().cloned().map(DraftImage::Recovered).collect();
            self.draft_changed();
        } else if let Some(images) = self.recall_images.take() {
            self.draft_images = images;
            self.draft_changed();
        }
    }

    pub(crate) fn draft_changed(&mut self) {
        if self.draft.recalled_images().is_none() {
            self.recall_images = None;
        }
        self.draft_revision = self.draft_revision.wrapping_add(1);
        self.draft_images
            .retain(|image| self.draft.retains_atom(image.label()));
    }

    pub(super) fn restore_draft_images(&mut self, text: &str, images: Vec<ImageAttachment>) {
        let mut restored = images
            .into_iter()
            .filter_map(|image| {
                let range = image.marker.range_in(text).or_else(|| {
                    (image.marker.offset == text.len()
                        && !image.marker.label.is_empty()
                        && !image.marker.label.contains(['\n', '\r']))
                    .then_some(text.len()..text.len())
                })?;
                Some((range, DraftImage::Recovered(image)))
            })
            .collect::<Vec<_>>();
        restored.sort_by_key(|(range, _)| range.start);

        if !self.draft.text().is_empty() {
            self.draft.append("\n\n");
        }
        let mut offset = 0;
        for (range, mut entry) in restored {
            if range.start < offset {
                continue;
            }
            self.draft.append(&text[offset..range.start]);
            if range.is_empty() && !self.draft.text().is_empty() {
                self.draft.append(" ");
            }
            if self.draft.atom_range(entry.label()).is_some() {
                let label = loop {
                    let label = self.next_image_label();
                    if !text.contains(&label) {
                        break label;
                    }
                };
                entry.rename(label);
            }
            self.draft.insert_atom(entry.label());
            self.draft_images.push(entry);
            offset = range.end;
        }
        self.draft.append(&text[offset..]);
        self.draft_changed();
    }

    pub(crate) fn image_activity(&self) -> Option<&'static str> {
        if self.draft.recalled_image_event().is_some() {
            Some("Loading prompt images…")
        } else if self.clipboard_ticket.is_some() {
            Some("Reading clipboard…")
        } else if self.image_send.is_some() {
            Some("Preparing images…")
        } else {
            None
        }
    }

    pub(crate) fn has_transparent_images(&self) -> bool {
        self.attached_images().any(DraftImage::transparent)
    }

    pub(crate) fn attached_images(&self) -> impl Iterator<Item = &DraftImage> {
        self.draft_images
            .iter()
            .filter(|image| self.draft.atom_range(image.label()).is_some())
    }

    fn next_image_label(&mut self) -> String {
        loop {
            self.next_image += 1;
            let label = format!("[Image #{}]", self.next_image);
            if !self.draft.text().contains(&label) {
                return label;
            }
        }
    }

    pub(crate) fn clear_draft_images(&mut self) {
        self.draft_images.clear();
        self.recall_images = None;
        self.next_image = 0;
        self.encoded_images.clear();
        self.image_send = None;
        self.clipboard_ticket = None;
    }
}

#[cfg(test)]
mod tests {
    use kuku_client::{ImageAttachment, ImageMarker};

    use super::{Address, App, DraftImage, Pane};

    #[test]
    fn recalled_image_lookup_finishes_in_the_pane_that_started_it() {
        let mut app = App::new(std::path::PathBuf::from("/workspace"), None);
        let address = app.active.clone();
        let caption = "[Image #1]";
        app.pane_mut().draft.remember_image_event(caption, 42);
        app.pane_mut().recall_draft(-1);
        let selection = app.selection();

        let other = Address::new(address.session.clone(), "other".into());
        app.select(other, true, String::new());
        app.recalled_image_draft(
            selection,
            42,
            Ok(kuku_client::MessageDraft {
                text: caption.into(),
                images: vec![ImageAttachment {
                    marker: ImageMarker {
                        offset: 0,
                        label: caption.into(),
                    },
                    data: "saved image bytes".into(),
                    transparent: false,
                }],
            }),
        );
        assert!(app.pane().draft.text().is_empty());

        app.select(address, true, String::new());
        assert_eq!(app.pane().draft.text(), caption);
        assert_eq!(app.pane().draft.recalled_image_event(), None);
        assert_eq!(app.pane().attached_images().count(), 1);
    }

    #[test]
    fn restored_send_preserves_attachment_ranges_in_another_draft() {
        let mut pane = Pane::default();
        let image = |data: &str, offset| ImageAttachment {
            marker: ImageMarker {
                offset,
                label: "[Image #1]".into(),
            },
            data: data.to_owned(),
            transparent: false,
        };
        let text = "literal [Image #1] [Image #1]";
        let rejected = image("rejected image", "literal [Image #1] ".len());

        let existing = image("current image", 0);
        pane.draft.insert_atom(&existing.marker.label);
        pane.draft_images.push(DraftImage::Recovered(existing));
        pane.restore_draft_images(text, vec![rejected]);

        assert_eq!(
            pane.draft.text(),
            "[Image #1]\n\nliteral [Image #1] [Image #2]"
        );
        assert_eq!(pane.draft.atom_range("[Image #1]"), Some(0..10));
        let restored = pane.draft.text().find("[Image #2]").unwrap();
        assert_eq!(
            pane.draft.atom_range("[Image #2]"),
            Some(restored..restored + 10)
        );
        assert_eq!(pane.attached_images().count(), 2);
    }
}
