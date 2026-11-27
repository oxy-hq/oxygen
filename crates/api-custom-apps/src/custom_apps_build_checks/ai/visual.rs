//! Asking the model about screens it is shown pictures of: the changed
//! screens' note and the design audit.
//!
//! **Every picture, in as many requests as it takes.** The only bounds are the
//! ones the model's provider documents (`ImageLimits`: pictures a request, the
//! size of one, the size of the request). Screens are taken in order; a
//! request is filled until the next screen's pictures would not fit, then it
//! is sent and the next one started. A screen's pictures always ride together,
//! and one request's worth of pictures is in memory at a time. Nothing is
//! downscaled: the workspace has no image decoder, so a picture is sent as it
//! was kept or not at all. The answers are joined in order.
//!
//! **When the pictures are not taken, the words still are.** A provider with
//! no image input is never sent one. A provider that refuses a request that
//! carried pictures is asked the same thing again without them. **Only a
//! refusal that says the model takes no pictures turns them off** for the
//! requests after it (`AskError::PicturesNotTaken`); any other refusal (a
//! prompt too long with them, a picture the provider will not read) is that
//! request's alone, and the next one carries its pictures as before. What the
//! model was not shown is counted, and the note says so
//! ([`Asked::unseen_line`]).

use agentic_pipeline::one_shot::{ImageInput, ImageLimits};

use super::inputs::Files;
use super::model::{AskError, NoteModel};

/// The most bytes of material one request carries beside its pictures: what
/// keeps a request inside a model's context when many screens changed. More
/// screens than that go in the next request; none is left out.
pub const MAX_REQUEST_TEXT_BYTES: usize = 150_000;

/// One screen the model is asked about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subject {
    /// What the model is told about it, already in its `<data>` blocks.
    pub words: String,
    /// Its kept pictures: each file's name, and the line that names it.
    pub pictures: Vec<(String, String)>,
}

/// What is asked.
#[derive(Clone, Copy, Debug)]
pub struct Question<'a> {
    pub system: &'a str,
    /// What opens the first request (and the only one, when one is enough).
    pub opening: &'a str,
    /// What opens each later request, in place of `opening`.
    pub continuing: &'a str,
    pub subjects: &'a [Subject],
}

/// Why the model was not shown a screen's pictures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unseen {
    /// Its provider has no image input.
    NoImageInput,
    /// Its provider refused a request that carried pictures.
    Refused,
    /// No picture of the screen was kept, or none a request can carry.
    NoneKept,
}

/// What the model answered, and what it was shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asked {
    /// The answers, joined in order.
    pub text: String,
    /// How many requests it took.
    pub requests: usize,
    /// The screens that have a picture to show.
    pub pictured: usize,
    /// Of those, the ones whose pictures the model was sent.
    pub seen: usize,
    /// Why the rest were not sent, when some were not.
    pub unseen: Option<Unseen>,
}

impl Asked {
    /// The line a note opens with when the model was not shown every picture
    /// there was to show; `None` when it saw them all, or there were none to
    /// begin with and nothing claimed otherwise.
    pub fn unseen_line(&self) -> Option<String> {
        let why = match self.unseen? {
            Unseen::NoImageInput => "this workspace's model takes no pictures",
            Unseen::Refused => "the model's provider refused them",
            Unseen::NoneKept => "no picture of them was kept that could be sent",
        };
        let scope = if self.seen == 0 {
            "The model did not see the pictures".to_string()
        } else {
            format!(
                "The model saw the pictures of {} of {} screens only",
                self.seen, self.pictured
            )
        };
        Some(format!(
            "{scope} ({why}): this is written from each screen's text and numbers."
        ))
    }
}

/// One request being filled.
#[derive(Default)]
struct Batch {
    /// Indexes into the question's subjects.
    subjects: Vec<usize>,
    images: Vec<ImageInput>,
    /// How many of `subjects` have a picture in `images`.
    pictured: usize,
    /// The bytes of the subjects' words.
    bytes: usize,
}

/// The asking, as it goes.
struct Asking<'a> {
    model: &'a dyn NoteModel,
    files: &'a Files<'a>,
    question: Question<'a>,
    /// `None` once pictures are off: the provider takes none, or refused some.
    limits: Option<ImageLimits>,
    answers: Vec<String>,
    pictured: usize,
    seen: usize,
    unseen: Option<Unseen>,
}

impl Asking<'_> {
    /// The pictures of `subject` a request can carry, labelled; none once
    /// pictures are off. A subject with a picture kept counts as pictured.
    async fn pictures_of(&mut self, subject: &Subject) -> Vec<ImageInput> {
        let Some(limits) = self.limits else {
            return Vec::new();
        };
        let mut images = Vec::new();
        for (name, label) in &subject.pictures {
            if let Some(bytes) = self.files.picture(name).await {
                let image = ImageInput::png(bytes).labelled(label.as_str());
                if limits.refuses(&image).is_none() {
                    images.push(image);
                }
            }
        }
        if images.is_empty() && !subject.pictures.is_empty() {
            self.unseen.get_or_insert(Unseen::NoneKept);
        }
        images
    }

    /// Whether `batch` can take a subject of `bytes` of words and `images`.
    fn fits(&self, batch: &Batch, bytes: usize, images: &[ImageInput]) -> bool {
        let text = batch.bytes + bytes;
        let carried = self
            .limits
            .is_none_or(|limits| limits.can_carry(&batch.images, images, text));
        text <= MAX_REQUEST_TEXT_BYTES && carried
    }

    /// The words of one request: how it opens, then each subject of `batch`
    /// with whether its pictures are attached.
    fn text(&self, batch: &Batch, with_pictures: bool) -> String {
        let opening = if self.answers.is_empty() {
            self.question.opening
        } else {
            self.question.continuing
        };
        let mut parts = vec![opening.to_string()];
        for index in &batch.subjects {
            parts.push(self.question.subjects[*index].words.clone());
        }
        parts.push(if with_pictures && !batch.images.is_empty() {
            format!(
                "{} pictures are attached above, each named by the line before it.",
                batch.images.len()
            )
        } else {
            "No picture is attached to this request: judge from the text and numbers alone."
                .to_string()
        });
        parts.join("\n\n")
    }

    /// Send `batch`. A refusal of the request that carried its pictures asks
    /// the same again without them; pictures are turned off for the requests
    /// after it only when the refusal says the model takes none.
    async fn send(&mut self, batch: Batch) -> Result<(), AskError> {
        let system = self.question.system;
        let text = self.text(&batch, true);
        let answer = match self.model.ask(system, &text, &batch.images).await {
            Ok(answer) => {
                self.seen += batch.pictured;
                answer
            }
            Err(why) if !batch.images.is_empty() && refused_with_pictures(why) => {
                if why == AskError::PicturesNotTaken {
                    self.limits = None;
                }
                self.unseen = Some(Unseen::Refused);
                let text = self.text(&batch, false);
                self.model.ask(system, &text, &[]).await?
            }
            Err(e) => return Err(e),
        };
        self.answers.push(answer.trim().to_string());
        Ok(())
    }

    fn finish(self) -> Asked {
        Asked {
            requests: self.answers.len(),
            text: self.answers.join("\n\n"),
            pictured: self.pictured,
            seen: self.seen,
            unseen: self.unseen.filter(|_| self.seen < self.pictured),
        }
    }
}

/// Whether a failure of a request that carried pictures may be the pictures'
/// doing: the provider takes none, or it refused this one request. The words
/// are then asked again alone, and a failure of that is the call's.
fn refused_with_pictures(why: AskError) -> bool {
    why == AskError::PicturesNotTaken || why.refused_this_request()
}

/// Ask `question` of `model`, showing it every picture of every subject its
/// provider's bounds allow, in as many requests as that takes (module docs).
/// `Err` is the call that failed; a refusal of the pictures is not one.
pub async fn ask(
    model: &dyn NoteModel,
    files: &Files<'_>,
    question: Question<'_>,
) -> Result<Asked, AskError> {
    let limits = model.image_limits();
    let mut asking = Asking {
        model,
        files,
        question,
        limits,
        answers: Vec::new(),
        pictured: 0,
        seen: 0,
        unseen: limits.is_none().then_some(Unseen::NoImageInput),
    };
    let mut batch = Batch::default();
    for (index, subject) in question.subjects.iter().enumerate() {
        asking.pictured += usize::from(!subject.pictures.is_empty());
        let mut images = asking.pictures_of(subject).await;
        let bytes = subject.words.len();
        if !batch.subjects.is_empty() && !asking.fits(&batch, bytes, &images) {
            asking.send(std::mem::take(&mut batch)).await?;
            if asking.limits.is_none() {
                // That request's pictures were refused: none goes out again.
                images.clear();
            }
        }
        batch.subjects.push(index);
        batch.pictured += usize::from(!images.is_empty());
        batch.bytes += bytes;
        batch.images.extend(images);
    }
    if !batch.subjects.is_empty() || asking.answers.is_empty() {
        asking.send(batch).await?;
    }
    Ok(asking.finish())
}
