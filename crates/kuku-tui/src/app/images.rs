use std::sync::Arc;
use std::time::{Duration, Instant};

use kuku_client::{ImageAttachment, MAX_INPUT_IMAGES, Timing};

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
            Self::Recovered(image) => &image.label,
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
            Self::Recovered(image) => image.label = label,
        }
    }
}

pub(crate) struct PrepareImages {
    pub(crate) ticket: u64,
    pub(crate) entries: Vec<DraftImage>,
    pub(crate) original: bool,
}

impl PrepareImages {
    pub(crate) fn encode(self) -> Result<Vec<ImageAttachment>, String> {
        self.entries
            .into_iter()
            .map(|entry| match entry {
                DraftImage::Recovered(image) => Ok(image),
                DraftImage::Clipboard {
                    label,
                    source,
                    transparent,
                } => {
                    let data = source.encode(self.original)?;
                    Ok(ImageAttachment {
                        label,
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
                    pane.draft_revision = pane.draft_revision.wrapping_add(1);
                }
            }
            Ok(Pasted::Image(source)) if pane.attached_images().count() < MAX_INPUT_IMAGES => {
                let draft = pane.draft.text();
                pane.draft_images
                    .retain(|image| draft.contains(image.label()));
                let label = pane.next_image_label();
                let transparent = source.transparent;
                let preceding = &pane.draft.text()[..pane.draft.cursor()];
                let separator = if preceding.ends_with(char::is_whitespace) || preceding.is_empty()
                {
                    ""
                } else {
                    " "
                };
                let marker = format!("{separator}{label} ");
                pane.draft.insert(&marker);
                pane.draft_revision = pane.draft_revision.wrapping_add(1);
                pane.draft_images.push(DraftImage::Clipboard {
                    label,
                    source: Arc::new(source),
                    transparent,
                });
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
        let entries: Vec<_> = pane.attached_images().cloned().collect();
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
        if pane.clipboard_ticket.is_some() || pane.image_send.is_some() {
            return true;
        }
        if !pane.encoded_images.is_empty() || pane.attached_images().next().is_none() {
            return false;
        }
        let known_capabilities = if let Some(snapshot) = &pane.snapshot {
            snapshot
                .pending_selection
                .as_ref()
                .or(snapshot.selection.as_ref())
                .map(|selection| &selection.capabilities)
        } else {
            pane.initial_model.as_ref().map(|model| &model.capabilities)
        };
        if known_capabilities.is_some_and(|capabilities| {
            capabilities.media_adapter
                != kuku::config::MediaAdapterSupport::ResponsesImageToolResults
        }) {
            self.problem(
                "This model cannot receive images · switch models or remove image markers",
            );
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
    pub(super) fn restore_draft_images(
        &mut self,
        text: &str,
        images: Vec<ImageAttachment>,
    ) -> String {
        let mut text = text.to_owned();
        for image in images {
            let mut entry = self
                .sent_images
                .iter()
                .find(|draft| draft.label() == image.label)
                .cloned()
                .unwrap_or(DraftImage::Recovered(image));
            if self.draft.text().contains(entry.label()) {
                let label = self.next_image_label();
                text = text.replace(entry.label(), &label);
                entry.rename(label);
            }
            self.draft_images.push(entry);
        }
        self.sent_images.clear();
        text
    }

    pub(crate) fn image_activity(&self) -> Option<&'static str> {
        if self.clipboard_ticket.is_some() {
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
            .filter(|image| self.draft.text().contains(image.label()))
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
        self.next_image = 0;
        self.encoded_images.clear();
        self.image_send = None;
        self.clipboard_ticket = None;
    }
}
