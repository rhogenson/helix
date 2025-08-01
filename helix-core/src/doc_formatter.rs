//! The `DocumentFormatter` forms the bridge between the raw document text
//! and onscreen positioning. It yields the text graphemes as an iterator
//! and traverses (part) of the document text. During that traversal it
//! handles grapheme detection, softwrapping and annotations.
//! It yields `FormattedGrapheme`s and their corresponding visual coordinates.
//!
//! As both virtual text and softwrapping can insert additional lines into the document
//! it is generally not possible to find the start of the previous visual line.
//! Instead the `DocumentFormatter` starts at the last "checkpoint" (usually a linebreak)
//! called a "block" and the caller must advance it as needed.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::fmt::Debug;
use std::mem::replace;

#[cfg(test)]
mod test;

use unicode_segmentation::{Graphemes, UnicodeSegmentation};

use helix_stdx::rope::{RopeGraphemes, RopeSliceExt};

use crate::graphemes::{Grapheme, GraphemeStr};
use crate::syntax::Highlight;
use crate::text_annotations::TextAnnotations;
use crate::{movement, Change, LineEnding, Position, RopeSlice, Tendril};

/// TODO make Highlight a u32 to reduce the size of this enum to a single word.
#[derive(Debug, Clone, Copy)]
pub enum GraphemeSource {
    Document {
        codepoints: u32,
    },
    /// Inline virtual text can not be highlighted with a `Highlight` iterator
    /// because it's not part of the document. Instead the `Highlight`
    /// is emitted right by the document formatter
    VirtualText {
        highlight: Option<Highlight>,
    },
}

impl GraphemeSource {
    /// Returns whether this grapheme is virtual inline text
    pub fn is_virtual(self) -> bool {
        matches!(self, GraphemeSource::VirtualText { .. })
    }

    pub fn is_eof(self) -> bool {
        // all doc chars except the EOF char have non-zero codepoints
        matches!(self, GraphemeSource::Document { codepoints: 0 })
    }

    pub fn doc_chars(self) -> usize {
        match self {
            GraphemeSource::Document { codepoints } => codepoints as usize,
            GraphemeSource::VirtualText { .. } => 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FormattedGrapheme<'a> {
    pub raw: Grapheme<'a>,
    pub source: GraphemeSource,
    pub visual_pos: Position,
    /// Document line at the start of the grapheme
    pub line_idx: usize,
    /// Document char position at the start of the grapheme
    pub char_idx: usize,
}

impl FormattedGrapheme<'_> {
    pub fn is_virtual(&self) -> bool {
        self.source.is_virtual()
    }

    pub fn doc_chars(&self) -> usize {
        self.source.doc_chars()
    }

    pub fn is_whitespace(&self) -> bool {
        self.raw.is_whitespace()
    }

    pub fn width(&self) -> usize {
        self.raw.width()
    }

    pub fn is_word_boundary(&self) -> bool {
        self.raw.is_word_boundary()
    }
}

#[derive(Debug, Clone)]
struct GraphemeWithSource<'a> {
    grapheme: Grapheme<'a>,
    source: GraphemeSource,
}

impl<'a> GraphemeWithSource<'a> {
    fn new(
        g: GraphemeStr<'a>,
        visual_x: usize,
        tab_width: u16,
        source: GraphemeSource,
    ) -> GraphemeWithSource<'a> {
        GraphemeWithSource {
            grapheme: Grapheme::new(g, visual_x, tab_width),
            source,
        }
    }
    fn placeholder() -> Self {
        GraphemeWithSource {
            grapheme: Grapheme::Other { g: " ".into() },
            source: GraphemeSource::Document { codepoints: 0 },
        }
    }

    fn doc_chars(&self) -> usize {
        self.source.doc_chars()
    }

    fn is_whitespace(&self) -> bool {
        self.grapheme.is_whitespace()
    }

    fn is_newline(&self) -> bool {
        matches!(self.grapheme, Grapheme::Newline)
    }

    fn is_eof(&self) -> bool {
        self.source.is_eof()
    }

    fn width(&self) -> usize {
        self.grapheme.width()
    }
}

#[derive(Debug, Clone)]
pub struct TextFormat {
    pub soft_wrap: bool,
    pub tab_width: u16,
    pub max_wrap: u16,
    pub max_indent_retain: u16,
    pub wrap_indicator: Box<str>,
    pub wrap_indicator_highlight: Option<Highlight>,
    pub viewport_width: u16,
    pub soft_wrap_at_text_width: bool,
}

// test implementation is basically only used for testing or when softwrap is always disabled
impl Default for TextFormat {
    fn default() -> Self {
        TextFormat {
            soft_wrap: false,
            tab_width: 4,
            max_wrap: 3,
            max_indent_retain: 4,
            wrap_indicator: Box::from(" "),
            viewport_width: 17,
            wrap_indicator_highlight: None,
            soft_wrap_at_text_width: false,
        }
    }
}

fn wrap_allowed(x: Option<&Grapheme>, y: &Grapheme) -> bool {
    x.is_some_and(|x| x.is_word_boundary() && (x.is_whitespace() || !y.is_word_boundary()))
}

#[derive(Debug)]
pub struct DocumentFormatter<'t> {
    text_fmt: &'t TextFormat,
    annotations: &'t TextAnnotations<'t>,

    /// The visual position at the end of the last yielded word boundary
    visual_pos: Position,
    graphemes: RopeGraphemes<'t>,
    /// The character pos of the `graphemes` iter used for inserting annotations
    char_pos: usize,
    /// The line pos of the `graphemes` iter used for inserting annotations
    line_pos: usize,
    exhausted: bool,

    inline_annotation_graphemes: Option<(Graphemes<'t>, Option<Highlight>)>,

    // softwrap specific
    /// The indentation of the current line
    /// Is set to `None` if the indentation level is not yet known
    /// because no non-whitespace graphemes have been encountered yet
    indent_level: Option<usize>,
    /// In case a long word needs to be split a single grapheme might need to be wrapped
    /// while the rest of the word stays on the same line
    peeked_grapheme: Option<GraphemeWithSource<'t>>,
    /// A first-in first-out (fifo) buffer for the Graphemes of any given word
    word_buf: Vec<GraphemeWithSource<'t>>,
    /// The index of the next grapheme that will be yielded from the `word_buf`
    word_i: usize,
}

impl<'t> DocumentFormatter<'t> {
    /// Creates a new formatter at the last block before `char_idx`.
    /// A block is a chunk which always ends with a linebreak.
    /// This is usually just a normal line break.
    /// However very long lines are always wrapped at constant intervals that can be cheaply calculated
    /// to avoid pathological behaviour.
    pub fn new_at_prev_checkpoint(
        text: RopeSlice<'t>,
        text_fmt: &'t TextFormat,
        annotations: &'t TextAnnotations,
        char_idx: usize,
    ) -> Self {
        // TODO divide long lines into blocks to avoid bad performance for long lines
        let block_line_idx = text.char_to_line(char_idx.min(text.len_chars()));
        let block_char_idx = text.line_to_char(block_line_idx);
        annotations.reset_pos(block_char_idx);

        DocumentFormatter {
            text_fmt,
            annotations,
            visual_pos: Position { row: 0, col: 0 },
            graphemes: text.slice(block_char_idx..).graphemes(),
            char_pos: block_char_idx,
            exhausted: false,
            indent_level: None,
            peeked_grapheme: None,
            word_buf: Vec::with_capacity(64),
            word_i: 0,
            line_pos: block_line_idx,
            inline_annotation_graphemes: None,
        }
    }

    fn next_inline_annotation_grapheme(
        &mut self,
        char_pos: usize,
    ) -> Option<(&'t str, Option<Highlight>)> {
        loop {
            if let Some(&mut (ref mut annotation, highlight)) =
                self.inline_annotation_graphemes.as_mut()
            {
                if let Some(grapheme) = annotation.next() {
                    return Some((grapheme, highlight));
                }
            }

            if let Some((annotation, highlight)) =
                self.annotations.next_inline_annotation_at(char_pos)
            {
                self.inline_annotation_graphemes = Some((
                    UnicodeSegmentation::graphemes(&*annotation.text, true),
                    highlight,
                ))
            } else {
                return None;
            }
        }
    }

    fn advance_grapheme(&mut self, col: usize, char_pos: usize) -> Option<GraphemeWithSource<'t>> {
        let (grapheme, source) =
            if let Some((grapheme, highlight)) = self.next_inline_annotation_grapheme(char_pos) {
                (grapheme.into(), GraphemeSource::VirtualText { highlight })
            } else if let Some(grapheme) = self.graphemes.next() {
                let codepoints = grapheme.len_chars() as u32;

                let overlay = self.annotations.overlay_at(char_pos);
                let grapheme = match overlay {
                    Some((overlay, _)) => overlay.grapheme.as_str().into(),
                    None => Cow::from(grapheme).into(),
                };

                (grapheme, GraphemeSource::Document { codepoints })
            } else {
                if self.exhausted {
                    return None;
                }
                self.exhausted = true;
                // EOF grapheme is required for rendering
                // and correct position computations
                return Some(GraphemeWithSource {
                    grapheme: Grapheme::Other { g: " ".into() },
                    source: GraphemeSource::Document { codepoints: 0 },
                });
            };

        let grapheme = GraphemeWithSource::new(grapheme, col, self.text_fmt.tab_width, source);

        Some(grapheme)
    }

    /// Move a word to the next visual line
    fn wrap_word(&mut self) -> usize {
        // softwrap this word to the next line
        let indent_carry_over = if let Some(indent) = self.indent_level {
            if indent as u16 <= self.text_fmt.max_indent_retain {
                indent as u16
            } else {
                0
            }
        } else {
            // ensure the indent stays 0
            self.indent_level = Some(0);
            0
        };

        let virtual_lines =
            self.annotations
                .virtual_lines_at(self.char_pos, self.visual_pos, self.line_pos);
        self.visual_pos.col = indent_carry_over as usize;
        self.visual_pos.row += 1 + virtual_lines;
        let mut i = 0;
        let mut word_width = 0;
        let wrap_indicator = UnicodeSegmentation::graphemes(&*self.text_fmt.wrap_indicator, true)
            .map(|g| {
                i += 1;
                let grapheme = GraphemeWithSource::new(
                    g.into(),
                    self.visual_pos.col + word_width,
                    self.text_fmt.tab_width,
                    GraphemeSource::VirtualText {
                        highlight: self.text_fmt.wrap_indicator_highlight,
                    },
                );
                word_width += grapheme.width();
                grapheme
            });
        self.word_buf.splice(0..0, wrap_indicator);

        for grapheme in &mut self.word_buf[i..] {
            let visual_x = self.visual_pos.col + word_width;
            grapheme
                .grapheme
                .change_position(visual_x, self.text_fmt.tab_width);
            word_width += grapheme.width();
        }
        word_width
    }

    fn peek_grapheme(&mut self, col: usize, char_pos: usize) -> Option<&GraphemeWithSource<'t>> {
        match &mut self.peeked_grapheme {
            None => self.peeked_grapheme = self.advance_grapheme(col, char_pos),
            Some(peeked_grapheme) => peeked_grapheme
                .grapheme
                .change_position(col, self.text_fmt.tab_width),
        }
        self.peeked_grapheme.as_ref()
    }

    fn advance_to_next_word(&mut self) {
        self.word_buf.clear();
        let mut word_width = 0;
        let mut word_chars = 0;

        if self.exhausted && self.peeked_grapheme.is_none() {
            return;
        }

        loop {
            let mut col = self.visual_pos.col + word_width;
            let char_pos = self.char_pos + word_chars;
            match col.cmp(&(self.text_fmt.viewport_width as usize)) {
                // The EOF char and newline chars are always selectable in helix. That means
                // that wrapping happens "too-early" if a word fits a line perfectly. This
                // is intentional so that all selectable graphemes are always visible (and
                // therefore the cursor never disappears). However if the user manually set a
                // lower softwrap width then this is undesirable. Just increasing the viewport-
                // width by one doesn't work because if a line is wrapped multiple times then
                // some words may extend past the specified width.
                //
                // So we special case a word that ends exactly at line bounds and is followed
                // by a newline/eof character here.
                Ordering::Equal
                    if self.text_fmt.soft_wrap_at_text_width
                        && self
                            .peek_grapheme(col, char_pos)
                            .is_some_and(|grapheme| grapheme.is_newline() || grapheme.is_eof()) =>
                {
                    self.word_buf.push(self.peeked_grapheme.take().unwrap());
                    return;
                }
                Ordering::Equal if word_width > self.text_fmt.max_wrap as usize => return,
                Ordering::Greater if word_width > self.text_fmt.max_wrap as usize => {
                    self.peeked_grapheme = self.word_buf.pop();
                    return;
                }
                Ordering::Equal | Ordering::Greater => {
                    word_width = self.wrap_word();
                    col = self.visual_pos.col + word_width;
                }
                Ordering::Less => (),
            }

            // It would be nice if this could be written as
            //   let Some(grapheme) = self.peek_grapheme(col, char_pos)
            // but the borrow checker can't see inside of peek_grapheme and
            // assumes that the return value borrows mutably from self.
            let _ = self.peek_grapheme(col, char_pos);
            let Some(grapheme) = &self.peeked_grapheme else {
                return;
            };
            if wrap_allowed(
                self.word_buf.last().map(|g| &g.grapheme),
                &grapheme.grapheme,
            ) {
                return;
            }

            let grapheme = self.peeked_grapheme.take().unwrap();
            word_chars += grapheme.doc_chars();

            // Track indentation
            if !grapheme.is_whitespace() && self.indent_level.is_none() {
                self.indent_level = Some(self.visual_pos.col);
            } else if grapheme.grapheme == Grapheme::Newline {
                self.indent_level = None;
            }

            word_width += grapheme.width();
            self.word_buf.push(grapheme);
        }
    }

    /// returns the char index at the end of the last yielded grapheme
    pub fn next_char_pos(&self) -> usize {
        self.char_pos
    }
    /// returns the visual position at the end of the last yielded grapheme
    pub fn next_visual_pos(&self) -> Position {
        self.visual_pos
    }
}

impl<'t> Iterator for DocumentFormatter<'t> {
    type Item = FormattedGrapheme<'t>;

    fn next(&mut self) -> Option<Self::Item> {
        let grapheme = if self.text_fmt.soft_wrap {
            if self.word_i >= self.word_buf.len() {
                self.advance_to_next_word();
                self.word_i = 0;
            }
            let grapheme = replace(
                self.word_buf.get_mut(self.word_i)?,
                GraphemeWithSource::placeholder(),
            );
            self.word_i += 1;
            grapheme
        } else {
            self.advance_grapheme(self.visual_pos.col, self.char_pos)?
        };

        let grapheme = FormattedGrapheme {
            raw: grapheme.grapheme,
            source: grapheme.source,
            visual_pos: self.visual_pos,
            line_idx: self.line_pos,
            char_idx: self.char_pos,
        };

        self.char_pos += grapheme.doc_chars();
        if !grapheme.is_virtual() {
            self.annotations.process_virtual_text_anchors(&grapheme);
        }
        if grapheme.raw == Grapheme::Newline {
            // move to end of newline char
            self.visual_pos.col += 1;
            let virtual_lines =
                self.annotations
                    .virtual_lines_at(self.char_pos, self.visual_pos, self.line_pos);
            self.visual_pos.row += 1 + virtual_lines;
            self.visual_pos.col = 0;
            if !grapheme.is_virtual() {
                self.line_pos += 1;
            }
        } else {
            self.visual_pos.col += grapheme.width();
        }
        Some(grapheme)
    }
}

pub struct ReflowOpts<'a> {
    pub width: usize,
    pub line_ending: LineEnding,
    pub comment_tokens: &'a [String],
}

impl ReflowOpts<'_> {
    fn find_indent<'a>(&self, line: usize, doc: RopeSlice<'a>) -> RopeSlice<'a> {
        let line_start = doc.line_to_char(line);
        let mut indent_end = movement::skip_while(doc, line_start, |ch| matches!(ch, ' ' | '\t'))
            .unwrap_or(line_start);
        let slice = doc.slice(indent_end..);
        if let Some(token) = self
            .comment_tokens
            .iter()
            .filter(|token| slice.starts_with(token))
            .max_by_key(|x| x.len())
        {
            indent_end += token.chars().count();
        }
        let indent_end = movement::skip_while(doc, indent_end, |ch| matches!(ch, ' ' | '\t'))
            .unwrap_or(indent_end);
        return doc.slice(line_start..indent_end);
    }
}

/// reflow wraps long lines in text to be less than opts.width.
pub fn reflow(text: RopeSlice, char_pos: usize, opts: &ReflowOpts) -> Vec<Change> {
    // A constant so that reflow behaves consistently across
    // different configurations.
    const TAB_WIDTH: u16 = 8;

    let line_idx = text.char_to_line(char_pos.min(text.len_chars()));
    let mut char_pos = text.line_to_char(line_idx);

    let mut col = 0;
    let mut word_width = 0;
    let mut last_word_boundary = None;
    let mut changes = Vec::new();
    for grapheme in text.slice(char_pos..).graphemes() {
        let grapheme_chars = grapheme.len_chars();
        let mut grapheme = Grapheme::new(GraphemeStr::from(Cow::from(grapheme)), col, TAB_WIDTH);
        if col + grapheme.width() > opts.width && !grapheme.is_whitespace() {
            if let Some(n) = last_word_boundary {
                let indent = opts.find_indent(text.char_to_line(n), text);
                let mut whitespace_start = n;
                let mut whitespace_end = n;
                while whitespace_start > 0 && text.char(whitespace_start - 1) == ' ' {
                    whitespace_start -= 1;
                }
                while whitespace_end < text.chars().len() && text.char(whitespace_end) == ' ' {
                    whitespace_end += 1;
                }
                changes.push((
                    whitespace_start,
                    whitespace_end,
                    Some(Tendril::from(format!(
                        "{}{}",
                        opts.line_ending.as_str(),
                        &indent
                    ))),
                ));

                col = 0;
                for g in indent.graphemes() {
                    let g = Grapheme::new(GraphemeStr::from(Cow::from(g)), col, TAB_WIDTH);
                    col += g.width();
                }
                col += word_width;
                last_word_boundary = None;
                grapheme.change_position(col, TAB_WIDTH);
            }
        }
        col += grapheme.width();
        if grapheme == Grapheme::Newline {
            col = 0;
            word_width = 0;
            last_word_boundary = None;
        } else if grapheme.is_whitespace() {
            if word_width > 0 {
                last_word_boundary = Some(char_pos);
            }
            word_width = 0;
        } else {
            word_width += grapheme.width();
        }
        char_pos += grapheme_chars;
    }
    changes
}
