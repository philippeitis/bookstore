use std::borrow::Cow;

use regex::{Error as RegexError, Regex};
use sublime_fuzzy::best_match;

use bookstore_records::book::ColumnIdentifier;
use bookstore_records::Book;

// TODO: If search is too expensive, could sort searches by relative cost

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum SearchMode {
    Regex,
    ExactSubstring,
    Default,
    ExactString,
}

#[derive(Debug)]
pub enum Error {
    Regex(RegexError),
}

impl From<RegexError> for Error {
    fn from(e: RegexError) -> Self {
        Error::Regex(e)
    }
}
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Search {
    pub mode: SearchMode,
    pub column: ColumnIdentifier,
    pub search: String,
}

impl Search {
    pub(crate) fn into_matcher(self) -> Result<Box<dyn Matcher>, Error> {
        Ok(match self.mode {
            SearchMode::Regex => Box::new(RegexMatcher::new(self.column, self.search)?),
            SearchMode::ExactSubstring => {
                Box::new(ExactSubstringMatcher::new(self.column, self.search)?)
            }
            SearchMode::ExactString => Box::new(ExactStringMatcher::new(self.column, self.search)?),
            SearchMode::Default => Box::new(DefaultMatcher::new(self.column, self.search)?),
        })
    }
}

pub trait Matcher {
    fn new(column: ColumnIdentifier, search: String) -> Result<Self, Error>
    where
        Self: Sized;

    fn is_match(&self, book: &Book) -> bool;
}

pub struct RegexMatcher {
    column: ColumnIdentifier,
    regex: Regex,
}

impl Matcher for RegexMatcher {
    fn new(column: ColumnIdentifier, search: String) -> Result<Self, Error> {
        Ok(RegexMatcher {
            column,
            regex: Regex::new(&search)?,
        })
    }

    #[inline(always)]
    fn is_match(&self, book: &Book) -> bool {
        if self.column == ColumnIdentifier::Tags {
            return book.free_tags.iter().any(|v| self.regex.is_match(v))
                || self.regex.is_match("");
        }

        match book.get_column(&self.column) {
            None => self.regex.is_match(""),
            Some(Cow::Borrowed(value)) => self.regex.is_match(value),
            Some(Cow::Owned(value)) => self.regex.is_match(&value),
        }
    }
}

pub struct ExactSubstringMatcher {
    column: ColumnIdentifier,
    regex: Regex,
}

impl Matcher for ExactSubstringMatcher {
    fn new(column: ColumnIdentifier, search: String) -> Result<Self, Error> {
        Ok(ExactSubstringMatcher {
            column,
            regex: Regex::new(&regex::escape(&search))?,
        })
    }

    #[inline(always)]
    fn is_match(&self, book: &Book) -> bool {
        if self.column == ColumnIdentifier::Tags {
            return book.free_tags.iter().any(|v| self.regex.is_match(v))
                || self.regex.is_match("");
        }

        match book.get_column(&self.column) {
            None => self.regex.is_match(""),
            Some(Cow::Borrowed(value)) => self.regex.is_match(value),
            Some(Cow::Owned(value)) => self.regex.is_match(&value),
        }
    }
}

pub struct ExactStringMatcher {
    column: ColumnIdentifier,
    string: String,
}

impl Matcher for ExactStringMatcher {
    fn new(column: ColumnIdentifier, search: String) -> Result<Self, Error> {
        Ok(ExactStringMatcher {
            column,
            string: search,
        })
    }

    #[inline(always)]
    fn is_match(&self, book: &Book) -> bool {
        if self.column == ColumnIdentifier::Tags {
            return book.free_tags.contains(&self.string) || self.string.is_empty();
        }

        match book.get_column(&self.column) {
            None => self.string.is_empty(),
            Some(Cow::Borrowed(value)) => self.string == value,
            Some(Cow::Owned(value)) => self.string == value,
        }
    }
}

pub struct DefaultMatcher {
    column: ColumnIdentifier,
    string: String,
}

impl Matcher for DefaultMatcher {
    fn new(column: ColumnIdentifier, search: String) -> Result<Self, Error> {
        Ok(DefaultMatcher {
            column,
            string: search,
        })
    }

    #[inline(always)]
    fn is_match(&self, book: &Book) -> bool {
        if self.column == ColumnIdentifier::Tags {
            return book
                .free_tags
                .iter()
                .any(|value| best_match(&self.string, value).is_some())
                || best_match(&self.string, "").is_some();
        }

        match book.get_column(&self.column) {
            None => best_match(&self.string, ""),
            Some(Cow::Borrowed(value)) => best_match(&self.string, value),
            Some(Cow::Owned(value)) => best_match(&self.string, &value),
        }
        .is_some()
    }
}
