use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::ops::Range;
use std::path::Path;

use zip::{result::ZipError, ZipArchive};

use regex::{bytes::Regex as ByteRegex, Regex};

use quick_xml::{events::Event, Reader};
use std::fmt::Formatter;

// Not robust if string is escaped, but at the same time, who would do such a terrible thing in the
// root file?
fn get_root_file_byte_range(text: &[u8]) -> Option<Range<usize>> {
    lazy_static::lazy_static! {
        static ref RE: ByteRegex = ByteRegex::new(r#"(?:<rootfile )(?:[^>]*)(?:full-path=")([^"]*)(?:"[^>]*>)"#).unwrap();
    }
    let range = RE.captures(text)?.get(1)?;
    Some(range.start()..range.end())
}

fn get_isbn(text: &str) -> Option<String> {
    lazy_static::lazy_static! {
        static ref RE: Regex = Regex::new(r#"^(?:urn:isbn:)?([\d-]+)"#).unwrap();
    }
    Some(RE.captures(text)?.get(1)?.as_str().to_owned())
}

pub enum Error {
    BadZip,
    IoError,
    NoContainer,
    NoPackage,
    NoRootFile,
    // NoContent,
    // BadMimetype,
    // NoMimetype,
    NoMetadata,
    BadXML,
}

impl From<std::io::Error> for Error {
    fn from(_: std::io::Error) -> Self {
        Error::IoError
    }
}

impl From<ZipError> for Error {
    fn from(_: ZipError) -> Self {
        Error::BadZip
    }
}

impl From<quick_xml::Error> for Error {
    fn from(_: quick_xml::Error) -> Self {
        Error::BadXML
    }
}

pub struct Metadata {
    pub title: Option<String>,
    pub author: Option<String>,
    pub language: Option<String>,
    pub identifier: Option<(IdentifierScheme, String)>,
    pub description: Option<String>,
    pub extended_values: HashMap<String, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IdentifierScheme {
    Amazon,
    BarnesNoble,
    Calibre,
    EdelWeiss,
    FF,
    GoodReads,
    Google,
    ISBN,
    MOBIASIN,
    SonyBookID,
    URI,
    URL,
    URN,
    UUID,
    Unknown(String),
    None,
}

impl std::fmt::Display for IdentifierScheme {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                IdentifierScheme::Amazon => "amazon",
                IdentifierScheme::BarnesNoble => "barnesnoble",
                IdentifierScheme::Calibre => "calibre",
                IdentifierScheme::EdelWeiss => "edelweiss",
                IdentifierScheme::FF => "ff",
                IdentifierScheme::GoodReads => "goodreads",
                IdentifierScheme::Google => "google",
                IdentifierScheme::ISBN => "isbn",
                IdentifierScheme::MOBIASIN => "mobi-asin",
                IdentifierScheme::SonyBookID => "sonybookid",
                IdentifierScheme::URI => "uri",
                IdentifierScheme::URL => "url",
                IdentifierScheme::URN => "urn",
                IdentifierScheme::UUID => "uuid",
                IdentifierScheme::Unknown(id) => id,
                IdentifierScheme::None => "none",
            }
        )
    }
}

enum FieldSeen {
    Author,
    Title,
    // Publisher,
    Identifier(IdentifierScheme),
    Language,
    Description,
    Unknown(String),
}

// TODO: Can have multi-part titles.
impl Metadata {
    pub fn from_archive<R: std::io::Seek + std::io::Read>(
        archive: &mut ZipArchive<R>,
    ) -> Result<Self, Error> {
        // if let Ok(mut mime) = archive.by_name("mimetype") {
        //     let expected = b"application/epub+zip".to_vec();
        //     let mut buf = vec![0; expected.len()];
        //     mime.read_to_end(&mut buf)?;
        //     println!("{}", String::from_utf8_lossy(&buf));
        //     if buf != expected {
        //         return Err(EpubError::BadMimetype);
        //     }
        // } else {
        //     return Err(EpubError::NoMimetype);
        // }

        let root_file = if let Ok(mut meta_inf) = archive.by_name("META-INF/container.xml") {
            let mut buf = Vec::new();
            meta_inf.read_to_end(&mut buf)?;
            match get_root_file_byte_range(&buf) {
                Some(range) => String::from_utf8_lossy(&buf[range]).into_owned(),
                None => return Err(Error::NoRootFile),
            }
        } else {
            return Err(Error::NoContainer);
        };

        let meta_zip = archive
            .by_name(&root_file)
            .map_err(|_| Error::NoContainer)?;
        // TODO: 1KB seems to get most of the metadata. Validate these findings?
        let mut reader = Reader::from_reader(BufReader::with_capacity(2 << 10, meta_zip));
        reader.trim_text(true);

        let mut new_obj = Metadata {
            title: None,
            author: None,
            language: None,
            identifier: None,
            description: None,
            extended_values: HashMap::new(),
        };

        let mut buf = Vec::new();

        // Read possible declaration, as well as package tag.
        // Ignores comments and unexpected text.
        loop {
            match reader.read_event(&mut buf)? {
                Event::Start(e) => match e.name() {
                    b"opf:package" | b"package" => break,
                    _ => return Err(Error::NoPackage),
                },
                Event::Decl(_) => {
                    buf.clear();
                    match reader.read_event(&mut buf)? {
                        Event::Start(e) => match e.name() {
                            b"opf:package" | b"package" => break,
                            _ => return Err(Error::NoPackage),
                        },
                        _ => return Err(Error::NoPackage),
                    }
                }
                // We seem to have a case where we get a byte-order-mark
                // at the start, so we match text here too.
                Event::Text(_) | Event::Comment(_) => {}
                _ => return Err(Error::NoPackage),
            }
            buf.clear();
        }

        // Read metadata tag.
        // Ignores any comments.
        loop {
            match reader.read_event(&mut buf)? {
                Event::Start(e) => match e.name() {
                    b"metadata" | b"opf:metadata" => break,
                    _ => return Err(Error::NoMetadata),
                },
                Event::Comment(_) => {}
                _ => return Err(Error::NoMetadata),
            }
            buf.clear();
        }

        let mut seen = None;

        loop {
            match reader.read_event(&mut buf)? {
                Event::Start(ref e) => {
                    // println!(
                    //     "{} attributes values: {:?}",
                    //     String::from_utf8_lossy(e.name()),
                    //     e.attributes().map(|a| a.unwrap().value).collect::<Vec<_>>()
                    // );
                    seen = Some(match e.name() {
                        b"dc:creator" => FieldSeen::Author,
                        b"dc:title" => FieldSeen::Title,
                        b"dc:identifier" => {
                            let mut id = None;
                            let mut scheme = None;
                            for a in e.attributes().filter_map(Result::ok) {
                                match a.key {
                                    b"opf:scheme" => {
                                        scheme =
                                            Some(String::from_utf8_lossy(&a.value).into_owned())
                                    }
                                    b"id" => {
                                        id = Some(String::from_utf8_lossy(&a.value).into_owned())
                                    }
                                    _ => {}
                                }
                            }
                            FieldSeen::Identifier(match scheme {
                                None => match id {
                                    None => IdentifierScheme::None,
                                    Some(val) => match val.to_ascii_lowercase().as_str() {
                                        "uuid_id" => IdentifierScheme::UUID,
                                        "isbn" => IdentifierScheme::ISBN,
                                        id => IdentifierScheme::Unknown(id.to_owned()),
                                    },
                                },
                                Some(val) => match val.to_ascii_lowercase().as_str() {
                                    "amazon" => IdentifierScheme::Amazon,
                                    "barnesnoble" => IdentifierScheme::BarnesNoble,
                                    "calibre" => IdentifierScheme::Calibre,
                                    "edelweiss" => IdentifierScheme::EdelWeiss,
                                    "ff" => IdentifierScheme::FF,
                                    "goodreads" => IdentifierScheme::GoodReads,
                                    "google" => IdentifierScheme::Google,
                                    "isbn" => IdentifierScheme::ISBN,
                                    "mobi-asin" => IdentifierScheme::MOBIASIN,
                                    "sonybookid" => IdentifierScheme::SonyBookID,
                                    "uri" => IdentifierScheme::URI,
                                    "url" => IdentifierScheme::URL,
                                    "urn" => IdentifierScheme::URN,
                                    "uuid" => IdentifierScheme::UUID,
                                    id => IdentifierScheme::Unknown(id.to_owned()),
                                },
                            })
                        }
                        b"dc:language" => FieldSeen::Language,
                        // b"dc:publisher" => FieldSeen::Publisher,
                        b"dc:description" => FieldSeen::Description,
                        // TODO: Use strip_prefix when stable.
                        bytes => FieldSeen::Unknown({
                            String::from_utf8_lossy(if bytes.starts_with(b"dc:") {
                                &bytes[3..]
                            } else {
                                bytes
                            })
                            .into_owned()
                        }),
                    })
                }
                Event::Text(e) => {
                    let val = e.unescape_and_decode(&reader)?;
                    match std::mem::take(&mut seen) {
                        Some(FieldSeen::Author) => {
                            new_obj.author = Some(val);
                        }
                        Some(FieldSeen::Title) => {
                            new_obj.title = Some(val);
                        }
                        // Some(FieldSeen::Publisher) => {}
                        Some(FieldSeen::Identifier(scheme)) => match scheme {
                            IdentifierScheme::ISBN => {
                                new_obj.identifier =
                                    get_isbn(&val).map(|v| (IdentifierScheme::ISBN, v))
                            }
                            _ => {}
                        },
                        Some(FieldSeen::Language) => {
                            new_obj.language = Some(val);
                        }
                        Some(FieldSeen::Description) => {
                            new_obj.description = Some(val);
                        }
                        Some(FieldSeen::Unknown(value)) => {
                            new_obj.extended_values.insert(value, val);
                        }
                        None => {}
                    }
                }
                Event::End(e) => match e.name() {
                    b"metadata" | b"opf:metadata" => break,
                    _ => {}
                },
                Event::Eof => break,
                _ => {}
            }

            buf.clear();
        }

        Ok(new_obj)
    }

    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, Error> {
        let mut buf = BufReader::new(File::open(path)?);
        Self::from_archive(&mut ZipArchive::new(&mut buf)?)
    }
}

// TODO: https://www.oreilly.com/library/view/epub-3-best/9781449329129/ch01.html

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_get_isbn() {
        let isbn = "urn:isbn:0123456789012";
        assert_eq!(get_isbn(isbn), Some(String::from("0123456789012")));
        let isbn = "0123456789012";
        assert_eq!(get_isbn(isbn), Some(String::from("0123456789012")));
        let isbn = "978-0-345-53979-3";
        assert_eq!(get_isbn(isbn), Some(String::from("978-0-345-53979-3")));
        let isbn = "hello world:0123456789012";
        assert_eq!(get_isbn(isbn), None);
    }
}
