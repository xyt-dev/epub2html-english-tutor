use serde::{Deserialize, Serialize};

/// A single paragraph extracted from an epub chapter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Paragraph {
    /// Unique ID: "{book_slug}-ch{chapter:03}-p{para:04}"
    pub id: String,
    /// The raw English text of this paragraph.
    pub text: String,
}

/// A chapter (spine item) from an epub.
#[derive(Debug, Clone)]
pub struct Chapter {
    pub index: usize,
    pub title: Option<String>,
    pub paragraphs: Vec<Paragraph>,
}

/// A parsed book.
#[derive(Debug, Clone)]
pub struct Book {
    pub slug: String,
    pub title: String,
    pub chapters: Vec<Chapter>,
}

/// The structured response the LLM must return for each paragraph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmResponse {
    pub translation: String,
    pub vocabulary: Vec<VocabEntry>,
    pub chunks: Vec<ChunkEntry>,
}

/// An IELTS 6.5+ vocabulary entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VocabEntry {
    pub word: String,
    pub ipa: String,
    pub pos: String,
    pub cn: String,
    pub example: String,
}

/// A useful language chunk / collocations entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkEntry {
    pub chunk: String,
    pub cn: String,
    pub example: String,
}
