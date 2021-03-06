use std::collections::{HashMap, HashSet};
use std::convert::{TryFrom, TryInto};
use std::ffi::OsString;
use std::fmt::Formatter;
use std::num::NonZeroU64;
use std::ops::{Bound, RangeBounds};
#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};
#[cfg(windows)]
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, RwLock};

use futures::executor::block_on;
use itertools::Itertools;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions, Connection, SqliteConnection};
use unicase::UniCase;

use bookstore_records::book::{BookID, ColumnIdentifier};
use bookstore_records::series::Series;
use bookstore_records::{Book, BookVariant, ColumnOrder, Edit};

use crate::bookmap::BookCache;
use crate::search::Search;
use crate::{AppDatabase, DatabaseError, IndexableDatabase};

const CREATE_BOOKS: &str = r#"CREATE TABLE IF NOT EXISTS `books` (
`book_id` INTEGER NOT NULL PRIMARY KEY,
`title` TEXT DEFAULT NULL,
`series_name` TEXT DEFAULT NULL,
`series_id` REAL DEFAULT NULL
);"#;

// TODO: Fix inability to store multiple authors.
const CREATE_NAMED_TAGS: &str = r#"CREATE TABLE IF NOT EXISTS `named_tags` (
`name` TEXT NOT NULL,
`value` TEXT NOT NULL,
`book_id` INTEGER NOT NULL,
UNIQUE(`name`, `book_id`),
FOREIGN KEY(book_id) REFERENCES books(book_id)
    ON UPDATE CASCADE
    ON DELETE CASCADE
);"#;

const CREATE_FREE_TAGS: &str = r#"CREATE TABLE IF NOT EXISTS `free_tags` (
`value` TEXT NOT NULL,
`book_id` INTEGER NOT NULL,
UNIQUE(`value`, `book_id`),
FOREIGN KEY(book_id) REFERENCES books(book_id)
    ON UPDATE CASCADE
    ON DELETE CASCADE
);"#;

const CREATE_MULTIMAP_TAGS: &str = r#"CREATE TABLE IF NOT EXISTS `multimap_tags` (
`name` TEXT NOT NULL,
`value` TEXT NOT NULL,
`book_id` INTEGER NOT NULL,
UNIQUE(`name`, `value`, `book_id`),
FOREIGN KEY(book_id) REFERENCES books(book_id)
    ON UPDATE CASCADE
    ON DELETE CASCADE
);"#;

const CREATE_VARIANTS: &str = r#"CREATE TABLE IF NOT EXISTS `variants` (
`book_type` TEXT NOT NULL,
`path` BLOB NOT NULL,
`local_title` TEXT DEFAULT NULL,
`identifier` TEXT DEFAULT NULL,
`language` TEXT DEFAULT NULL,
`description` TEXT DEFAULT NULL,
`id` INTEGER DEFAULT NULL,
`hash` BLOB NOT NULL,
`file_size` INTEGER NOT NULL,
`book_id` INTEGER NOT NULL,
FOREIGN KEY(book_id) REFERENCES books(book_id)
    ON UPDATE CASCADE
    ON DELETE CASCADE
);"#;

macro_rules! execute_query {
    ($self:ident, $query_str:expr) => ({
        {
            block_on(async {
                sqlx::query!($query_str).execute(&mut $self.backend).await
            }).map_err(DatabaseError::Backend)
        }
    });
    ($self:ident, $query_str:expr, $($args:tt),*) => ({
        {
            block_on(async {
                sqlx::query!($query_str, $($args),*).execute(&mut $self.backend).await
            }).map_err(DatabaseError::Backend)
        }
    })
}

macro_rules! execute_query_str {
    ($self: ident, $query_str: expr) => {{
        block_on(async {
            sqlx::query($query_str.as_ref())
                .execute(&mut $self.backend)
                .await
        })
        .map_err(DatabaseError::Backend)
    }};
}

macro_rules! run_query_as {
    ($self:ident, $out_struct:path, $query_str:expr) => ({
        {
            block_on(async {
                sqlx::query_as!($out_struct, $query_str).fetch_all(&mut $self.backend).await
            }).map_err(DatabaseError::Backend)
        }
    });
    ($self:ident, $out_struct:path, $query_str:expr, $($args:tt),*) => ({
        {
            block_on(async {
                sqlx::query!($out_struct, $query_str, $($args),*).fetch_all(&mut $self.backend).await
            }).map_err(DatabaseError::Backend)
        }
    })
}

macro_rules! book {
    ($book: ident) => {
        $book.as_ref().read().unwrap()
    };
}

struct BookData {
    book_id: i64,
    title: Option<String>,
    series_name: Option<String>,
    series_id: Option<f32>,
}

struct VariantData {
    book_id: i64,
    book_type: String,
    path: Vec<u8>,
    local_title: Option<String>,
    language: Option<String>,
    identifier: Option<String>,
    description: Option<String>,
    id: Option<i64>,
    file_size: i64,
    hash: Vec<u8>,
}

struct NamedTagData {
    name: String,
    value: String,
    book_id: i64,
}

struct FreeTagData {
    value: String,
    book_id: i64,
}

#[cfg(windows)]
fn v8_to_v16(a: Vec<u8>) -> Vec<u16> {
    a.chunks(2)
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .collect()
}

#[cfg(windows)]
fn v16_to_v8(a: Vec<u16>) -> Vec<u8> {
    let mut v = Vec::with_capacity(a.len() * 2);
    for val in a.into_iter() {
        v.extend_from_slice(&val.to_be_bytes());
    }
    v
}

#[derive(Debug)]
enum DataError {
    NullPrimaryID,
    Serialize(String),
}

impl std::error::Error for DataError {}

impl std::fmt::Display for DataError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            DataError::NullPrimaryID => f.write_str("id was null - SQLite database corrupted?"),
            DataError::Serialize(reason) => f.write_str(reason),
        }
    }
}

impl TryFrom<BookData> for Book {
    type Error = DataError;

    fn try_from(bd: BookData) -> Result<Self, Self::Error> {
        Ok(Book {
            id: if let Ok(id) = NonZeroU64::try_from(bd.book_id as u64) {
                Some(id)
            } else {
                return Err(DataError::NullPrimaryID);
            },
            title: bd.title.clone(),
            series: {
                let series_id = bd.series_id;
                bd.series_name.map(|sn| Series {
                    name: sn,
                    index: series_id,
                })
            },
            ..Default::default()
        })
    }
}

impl TryFrom<VariantData> for BookVariant {
    type Error = DataError;

    fn try_from(vd: VariantData) -> Result<Self, Self::Error> {
        Ok(BookVariant {
            book_type: ron::from_str(&vd.book_type).map_err(|_| {
                DataError::Serialize(format!("Failed to parse book type: {}", vd.book_type))
            })?,
            #[cfg(unix)]
            path: PathBuf::from(OsString::from_vec(vd.path)),
            #[cfg(windows)]
            path: PathBuf::from(OsString::from_wide(&v8_to_v16(vd.path))),
            local_title: vd.local_title,
            identifier: vd
                .identifier
                .as_ref()
                .map(|s| ron::from_str(s).ok())
                .flatten(),
            language: vd.language,
            additional_authors: None,
            translators: None,
            description: vd.description,
            id: vd.id.map(|id| u32::try_from(id).unwrap()),
            hash: {
                let len = vd.hash.len();
                vd.hash.try_into().map_err(|_| {
                    DataError::Serialize(format!("Got {} byte hash, expected 32 byte hash", len))
                })?
            },
            file_size: vd.file_size as u64,
            free_tags: HashSet::new(),
            named_tags: HashMap::new(),
        })
    }
}

pub struct SQLiteDatabase {
    backend: SqliteConnection,
    local_cache: BookCache,
    path: PathBuf,
}

// TODO:
//  Start up is slow with lots of books - at 1M, 20S to open (+1s per 50k) - fix through lazy loading
//  Deletion of many book ids is very slow for large numbers - fix through lazy loading, using SQL queries?
//
impl SQLiteDatabase {
    fn load_books(&mut self) -> Result<(), DatabaseError<<SQLiteDatabase as AppDatabase>::Error>> {
        // TODO: Benchmark this for large databases with complex books.
        let raw_books = run_query_as!(self, BookData, "SELECT * FROM books")?;
        let raw_variants = run_query_as!(self, VariantData, "SELECT * FROM variants")?;
        let raw_named_tags = run_query_as!(self, NamedTagData, "SELECT * FROM named_tags")?;
        let raw_free_tags = run_query_as!(self, FreeTagData, "SELECT * FROM free_tags")?;
        let raw_multimap_tags = run_query_as!(self, NamedTagData, "SELECT * FROM multimap_tags")?;

        let mut books: HashMap<_, _> = raw_books
            .into_iter()
            .filter_map(|book_data: BookData| match Book::try_from(book_data) {
                Ok(book) => Some((book.id(), book)),
                Err(e) => {
                    eprintln!("Failed to read book from SQLite database: {}", e);
                    None
                }
            })
            .collect();

        for variant in raw_variants.into_iter() {
            let id = NonZeroU64::try_from(variant.book_id as u64).unwrap();
            let variant = match BookVariant::try_from(variant) {
                Ok(variant) => variant,
                Err(e) => {
                    eprintln!("Failed to read book variant from SQLite database: {}", e);
                    continue;
                }
            };
            if let Some(book) = books.get_mut(&id) {
                book.push_variant(variant);
            } else {
                // TODO: Decide what to do here, since schema dictates that variants are deleted with owning book.
                eprintln!(
                    "SQLite database may be corrupted. Found orphan variant for {}.",
                    id
                );
            }
        }

        // To get all columns: "SELECT DISTINCT tag_name FROM extended_tags"
        let mut prime_cols = HashSet::new();
        for &col in &["title", "authors", "series", "id", "description"] {
            prime_cols.insert(col.to_owned());
        }

        for tag in raw_named_tags.into_iter() {
            let id = NonZeroU64::try_from(tag.book_id as u64).unwrap();
            match books.get_mut(&id) {
                None => {
                    // TODO: Decide what to do here, since schema dictates that variants are deleted with owning book.
                    eprintln!(
                        "SQLite database may be corrupted. Found orphan variant for {}.",
                        id
                    );
                }
                Some(book) => {
                    if !prime_cols.contains(&tag.name) {
                        prime_cols.insert(tag.name.clone());
                    }

                    book.extend_column(&ColumnIdentifier::NamedTag(tag.name), tag.value)
                        .unwrap();
                }
            }
        }

        for tag in raw_free_tags.into_iter() {
            let id = NonZeroU64::try_from(tag.book_id as u64).unwrap();
            match books.get_mut(&id) {
                None => {
                    // TODO: Decide what to do here, since schema dictates that variants are deleted with owning book.
                    eprintln!(
                        "SQLite database may be corrupted. Found orphan variant for {}.",
                        id
                    );
                }
                Some(book) => {
                    book.extend_column(&ColumnIdentifier::Tags, tag.value)
                        .unwrap();
                }
            }
        }

        for tag in raw_multimap_tags.into_iter() {
            let id = NonZeroU64::try_from(tag.book_id as u64).unwrap();
            match books.get_mut(&id) {
                None => {
                    // TODO: Decide what to do here, since schema dictates that variants are deleted with owning book.
                    eprintln!(
                        "SQLite database may be corrupted. Found orphan variant for {}.",
                        id
                    );
                }
                Some(book) => match tag.name.as_str() {
                    "author" => book
                        .extend_column(&ColumnIdentifier::Author, tag.value)
                        .unwrap(),
                    name => {
                        if !prime_cols.contains(name) {
                            prime_cols.insert(name.to_string());
                        }
                        book.extend_column(
                            &ColumnIdentifier::MultiMap(name.to_string()),
                            tag.value,
                        )
                        .unwrap();
                    }
                },
            }
        }

        self.local_cache = BookCache::from_values_unchecked(
            books
                .into_iter()
                .map(|(a, b)| (a, Arc::new(RwLock::new(b))))
                .collect(),
            prime_cols.into_iter().map(UniCase::new).collect(),
        );
        Ok(())
    }
}

impl SQLiteDatabase {
    async fn insert_book_async(
        &mut self,
        book: BookVariant,
    ) -> Result<BookID, <Self as AppDatabase>::Error> {
        let ids = self.insert_books_async(vec![book], 1).await?;
        Ok(ids[0])
    }

    async fn insert_books_async<I: IntoIterator<Item = BookVariant>>(
        &mut self,
        books: I,
        transaction_size: usize,
    ) -> Result<Vec<BookID>, <Self as AppDatabase>::Error> {
        let mut book_iter = books.into_iter().peekable();
        let mut ids = Vec::with_capacity({
            let (low, high) = book_iter.size_hint();
            high.unwrap_or(low)
        });

        while book_iter.peek().is_some() {
            let mut tx = self.backend.begin().await?;
            for variant in book_iter.by_ref().take(transaction_size) {
                let variant: BookVariant = variant;
                let title = variant.local_title.as_ref();
                // let (series, series_index) = (None, None);
                // match book.get_series() {
                //     None => (None, None),
                //     Some(series) => (Some(&series.name), series.index.clone()),
                // };

                // let id = sqlx::query!(
                //     "INSERT into books (title, series_name, series_id) VALUES(?, ?, ?)",
                //     title,
                //     series,
                //     series_index
                // )
                // .execute(&mut tx)
                // .await?
                // .last_insert_rowid();

                let id = sqlx::query!("INSERT into books (title) VALUES(?)", title,)
                    .execute(&mut tx)
                    .await?
                    .last_insert_rowid();

                let book_type = ron::to_string(variant.book_type())
                    .expect("Serialization of value should never fail.");
                #[cfg(unix)]
                let path = variant.path().as_os_str().as_bytes();
                #[cfg(windows)]
                let path = v16_to_v8(variant.path().as_os_str().encode_wide().collect());
                let local_title = &variant.local_title;
                let identifier = variant
                    .identifier
                    .as_ref()
                    .map(|i| ron::to_string(i).expect("Serialization of value should never fail."));
                let language = &variant.language;
                let description = &variant.description;
                let sub_id = &variant.id;
                let hash = variant.hash.to_vec();
                let file_size = variant.file_size as i64;
                sqlx::query!(
                    "INSERT into variants (book_type, path, local_title, identifier, language, description, id, hash, file_size, book_id) VALUES(?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    book_type,
                    path,
                    local_title,
                    identifier,
                    language,
                    description,
                    sub_id,
                    hash,
                    file_size,
                    id,
                ).execute(&mut tx).await?;

                for value in variant.free_tags.iter() {
                    sqlx::query!(
                        "INSERT INTO free_tags (value, book_id) VALUES(?, ?);",
                        value,
                        id
                    )
                    .execute(&mut tx)
                    .await?;
                }

                for (name, value) in variant.named_tags.iter() {
                    sqlx::query!(
                        "INSERT INTO named_tags (name, value, book_id) VALUES(?, ?, ?);",
                        name,
                        value,
                        id
                    )
                    .execute(&mut tx)
                    .await?;
                }

                if let Some(authors) = &variant.additional_authors {
                    for author in authors {
                        sqlx::query!("INSERT INTO multimap_tags (name, value, book_id) VALUES(\"author\", ?, ?);", author, id).execute(&mut tx).await?;
                    }
                }

                let id = BookID::try_from(id as u64)
                    .expect("SQLite database should never return NULL ID from primary key.");
                self.local_cache
                    .insert_book(Book::from_variant(id, variant));

                ids.push(id);
            }
            tx.commit().await?;
        }
        Ok(ids)
    }

    async fn clear_db_async(&mut self) -> Result<(), sqlx::Error> {
        let mut tx = self.backend.begin().await?;
        sqlx::query!("DELETE FROM multimap_tags")
            .execute(&mut tx)
            .await?;
        sqlx::query!("DELETE FROM free_tags")
            .execute(&mut tx)
            .await?;
        sqlx::query!("DELETE FROM named_tags")
            .execute(&mut tx)
            .await?;
        sqlx::query!("DELETE FROM variants")
            .execute(&mut tx)
            .await?;
        sqlx::query!("DELETE FROM books").execute(&mut tx).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn remove_books_async<I: IntoIterator<Item = BookID>>(
        &mut self,
        merges: I,
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.backend.begin().await?;
        let merges = merges.into_iter();
        let size = {
            let (low, high) = merges.size_hint();
            high.unwrap_or(low)
        };
        sqlx::query(&format!("PRAGMA cache_size = {}", size))
            .execute(&mut tx)
            .await?;
        let s = merges
            .map(u64::from)
            .map(|x| (x as i64).to_string())
            .join(", ");
        sqlx::query(&format!("DELETE FROM books where book_id in ({})", s))
            .execute(&mut tx)
            .await?;
        sqlx::query!("PRAGMA cache_size = 2000")
            .execute(&mut tx)
            .await?;
        tx.commit().await
    }

    async fn merge_by_ids(&mut self, merges: &[(BookID, BookID)]) -> Result<(), sqlx::Error> {
        let mut tx = self.backend.begin().await?;
        for (merged_into, merged_from) in merges.iter().cloned() {
            let merged_into = u64::from(merged_into) as i64;
            let merged_from = u64::from(merged_from) as i64;

            sqlx::query!(
                "UPDATE OR IGNORE multimap_tags SET book_id = ? WHERE book_id = ?",
                merged_into,
                merged_from
            )
            .execute(&mut tx)
            .await?;

            // NOTE: Deletes orphan multimap tags.
            sqlx::query!("DELETE FROM multimap_tags WHERE book_id = ?", merged_from)
                .execute(&mut tx)
                .await?;

            sqlx::query!(
                "UPDATE named_tags SET book_id = ? WHERE book_id = ?",
                merged_into,
                merged_from
            )
            .execute(&mut tx)
            .await?;

            sqlx::query!(
                "UPDATE free_tags SET book_id = ? WHERE book_id = ?",
                merged_into,
                merged_from
            )
            .execute(&mut tx)
            .await?;

            sqlx::query!(
                "UPDATE variants SET book_id = ? WHERE book_id = ?",
                merged_into,
                merged_from
            )
            .execute(&mut tx)
            .await?;
        }
        tx.commit().await
    }

    async fn edit_book_by_id_async(
        &mut self,
        id: BookID,
        edits: &[(ColumnIdentifier, Edit)],
    ) -> Result<(), DatabaseError<<Self as AppDatabase>::Error>> {
        if !self.local_cache.edit_book_with_id(id, &edits)? {
            return Err(DatabaseError::BookNotFound(id));
        }
        let mut tx = self.backend.begin().await.map_err(DatabaseError::Backend)?;

        let book_id = u64::from(id) as i64;
        for (column, edit) in edits {
            match edit {
                Edit::Delete => {
                    match column {
                    ColumnIdentifier::Title => {
                        sqlx::query!("UPDATE books SET title = null WHERE book_id = ?;", book_id)
                    }
                    ColumnIdentifier::Author => sqlx::query!(
                        "DELETE FROM multimap_tags where book_id = ? AND name = 'author';",
                        book_id
                    ),
                    ColumnIdentifier::ID => {
                        unreachable!("Updating local cache should have errored before this could be reached.")
                    }
                    ColumnIdentifier::Series => sqlx::query!(
                        "UPDATE books SET series_name = null, series_id = null WHERE book_id = ?;",
                        book_id
                    ),
                    ColumnIdentifier::Variants => {
                        unreachable!("Updating local cache should have errored before this could be reached.")
                    }
                    ColumnIdentifier::Description => sqlx::query!(
                        "UPDATE variants SET description = null WHERE book_id = ?;",
                        book_id
                    ),
                    ColumnIdentifier::NamedTag(column) => {
                            sqlx::query!(
                            "DELETE FROM named_tags where book_id = ? and name = ?;",
                            book_id,
                            column
                        ).execute(&mut tx).await.map_err(DatabaseError::Backend)?;
                        continue;
                    },
                    ColumnIdentifier::ExactTag(tag) => {
                        sqlx::query!(
                        "DELETE FROM free_tags where book_id = ? AND value = ?;",
                        book_id,
                        tag
                        ).execute(&mut tx).await.map_err(DatabaseError::Backend)?;
                        continue;
                    }
                    ColumnIdentifier::MultiMap(_) | ColumnIdentifier::MultiMapExact(_, _) => unimplemented!("Deleting multimap tags not supported."),
                    ColumnIdentifier::Tags => {
                        sqlx::query!(
                            "DELETE FROM free_tags where book_id = ?;", book_id,
                        )
                    }
                    }.execute(&mut tx).await.map_err(DatabaseError::Backend)?;
                }
                Edit::Replace(value) => match column {
                    ColumnIdentifier::Title => {
                        sqlx::query!(
                            "UPDATE books SET title = ? WHERE book_id = ?;",
                            value,
                            book_id
                        )
                        .execute(&mut tx)
                        .await
                        .map_err(DatabaseError::Backend)?;
                    }
                    ColumnIdentifier::Author => {
                        sqlx::query!(
                            "INSERT INTO multimap_tags (name, value, book_id) VALUES('author', ?, ?);",
                            value,
                            book_id
                        )
                        .execute(&mut tx)
                        .await
                        .map_err(DatabaseError::Backend)?;
                    }
                    ColumnIdentifier::Series => {
                        let series = Series::from_str(value).ok();
                        let (series, series_index) = match series {
                            None => (None, None),
                            Some(Series { name, index }) => (Some(name), index),
                        };

                        sqlx::query!(
                            "UPDATE books SET series_name = ?, series_id = ? WHERE book_id = ?",
                            series,
                            series_index,
                            book_id
                        )
                        .execute(&mut tx)
                        .await
                        .map_err(DatabaseError::Backend)?;
                    }
                    ColumnIdentifier::ID => {
                        unreachable!("id is immutable, and this case is reached when local cache is modified");
                    }
                    ColumnIdentifier::Variants => {
                        unreachable!(
                            "variants is immutable, and this case is reached when local cache is modified"
                        );
                    }
                    ColumnIdentifier::Description => {
                        sqlx::query!(
                            "UPDATE variants SET description = ? WHERE book_id = ?",
                            value,
                            book_id
                        )
                        .execute(&mut tx)
                        .await
                        .map_err(DatabaseError::Backend)?;
                    }
                    ColumnIdentifier::Tags => {
                        sqlx::query!(
                            "INSERT into free_tags (value, book_id) VALUES(?, ?)",
                            value,
                            book_id
                        )
                        .execute(&mut tx)
                        .await
                        .map_err(DatabaseError::Backend)?;
                    }
                    ColumnIdentifier::NamedTag(column) => {
                        sqlx::query!(
                            "INSERT into named_tags (name, value, book_id) VALUES(?, ?, ?)",
                            column,
                            value,
                            book_id
                        )
                        .execute(&mut tx)
                        .await
                        .map_err(DatabaseError::Backend)?;
                    }
                    ColumnIdentifier::ExactTag(tag) => {
                        sqlx::query!(
                            "DELETE FROM free_tags WHERE value = ? AND book_id = ?; INSERT into free_tags (value, book_id) VALUES(?, ?)",
                            tag,
                            book_id,
                            value,
                            book_id
                        )
                        .execute(&mut tx)
                        .await
                        .map_err(DatabaseError::Backend)?;
                    }
                    ColumnIdentifier::MultiMap(_) | ColumnIdentifier::MultiMapExact(_, _) => {
                        unimplemented!("Replacing multimap tags not supported.")
                    }
                },
                Edit::Append(value) => match column {
                    ColumnIdentifier::Title => {
                        sqlx::query!(
                            "UPDATE books SET title = title || ? WHERE book_id = ?;",
                            value,
                            book_id
                        )
                        .execute(&mut tx)
                        .await
                        .map_err(DatabaseError::Backend)?;
                    }
                    ColumnIdentifier::Author => {
                        sqlx::query!(
                            "INSERT INTO multimap_tags (name, value, book_id) VALUES('author', ?, ?);",
                            value,
                            book_id
                        )
                        .execute(&mut tx)
                        .await
                        .map_err(DatabaseError::Backend)?;
                    }
                    ColumnIdentifier::Series => {
                        unreachable!("book should reject concatenating to series");
                    }
                    ColumnIdentifier::ID => {
                        unreachable!("id is immutable, and this case is reached when local cache is modified");
                    }
                    ColumnIdentifier::Variants => {
                        unreachable!(
                            "variants is immutable, and this case is reached when local cache is modified"
                        );
                    }
                    ColumnIdentifier::Description => {
                        sqlx::query!(
                            "UPDATE variants SET description = description || ? WHERE book_id = ?",
                            value,
                            book_id
                        )
                        .execute(&mut tx)
                        .await
                        .map_err(DatabaseError::Backend)?;
                    }
                    ColumnIdentifier::Tags => {
                        sqlx::query!(
                            "INSERT into free_tags (value, book_id) VALUES(?, ?)",
                            value,
                            book_id
                        )
                        .execute(&mut tx)
                        .await
                        .map_err(DatabaseError::Backend)?;
                    }
                    ColumnIdentifier::NamedTag(column) => {
                        sqlx::query!(
                            "INSERT OR REPLACE INTO named_tags (name, value, book_id) VALUES(?, \
                            COALESCE((SELECT value from named_tags where name = ? AND book_id = ?), \'\') || ?, ?)",
                            column,
                            column,
                            book_id,
                            value,
                            book_id
                        )
                        .execute(&mut tx)
                        .await
                        .map_err(DatabaseError::Backend)?;
                    }
                    ColumnIdentifier::ExactTag(tag) => {
                        let new_tag = tag.to_owned() + value;
                        sqlx::query!(
                            "DELETE FROM free_tags where value = ? AND book_id = ?; INSERT into free_tags (value, book_id) VALUES(?, ?)",
                            tag,
                            book_id,
                            new_tag,
                            book_id
                        )
                        .execute(&mut tx)
                        .await
                        .map_err(DatabaseError::Backend)?;
                    }
                    ColumnIdentifier::MultiMap(_) | ColumnIdentifier::MultiMapExact(_, _) => {
                        unimplemented!("Appending to multimap tags not supported.")
                    }
                },
            }
        }
        tx.commit().await.map_err(DatabaseError::Backend)
    }

    async fn update_books_async<I: IntoIterator<Item = BookVariant>>(
        &mut self,
        _books: I,
        _transaction_size: usize,
    ) -> Result<Vec<BookID>, <Self as AppDatabase>::Error> {
        // Get file sizes and hashes
        // let books: Vec<BookVariant> = books.into_iter().collect();
        // let sizes_and_hashes: Vec<_> = books.iter().map(|b| (b.file_size, b.hash)).collect();
        unimplemented!();
    }
}

// TODO: Should we use a separate process to mirror changes to SQL database?
//  Would push writes to queue:
//  DELETE_ALL should clear queue, since everything will be deleted.
//  DELETE_BOOK_ID should clear anything that overwrites given book, except when
//  an ordering is enforced in previous command.
impl AppDatabase for SQLiteDatabase {
    type Error = sqlx::Error;

    fn open<P>(file_path: P) -> Result<Self, DatabaseError<Self::Error>>
    where
        P: AsRef<Path>,
        Self: Sized,
    {
        let db_exists = file_path.as_ref().exists();
        if !db_exists {
            if let Some(path) = file_path.as_ref().parent() {
                std::fs::create_dir_all(path)?;
            }
        }
        let database = block_on(async {
            SqliteConnectOptions::new()
                .filename(&file_path)
                .create_if_missing(true)
                .connect()
                .await
        })
        .map_err(DatabaseError::Backend)?;

        let mut db = Self {
            backend: database,
            local_cache: BookCache::default(),
            path: file_path.as_ref().to_path_buf(),
        };

        execute_query_str!(db, CREATE_BOOKS)?;
        execute_query_str!(db, CREATE_FREE_TAGS)?;
        execute_query_str!(db, CREATE_NAMED_TAGS)?;
        execute_query_str!(db, CREATE_MULTIMAP_TAGS)?;
        execute_query_str!(db, CREATE_VARIANTS)?;
        if db_exists {
            db.load_books()?;
        }
        Ok(db)
    }

    fn path(&self) -> &Path {
        self.path.as_path()
    }

    fn save(&mut self) -> Result<(), DatabaseError<Self::Error>> {
        Ok(())
    }

    fn insert_book(&mut self, book: BookVariant) -> Result<BookID, DatabaseError<Self::Error>> {
        block_on(self.insert_book_async(book)).map_err(DatabaseError::Backend)
    }

    fn insert_books<I: IntoIterator<Item = BookVariant>>(
        &mut self,
        books: I,
    ) -> Result<Vec<BookID>, DatabaseError<Self::Error>> {
        block_on(self.insert_books_async(books, 5000)).map_err(DatabaseError::Backend)
    }

    fn remove_book(&mut self, id: BookID) -> Result<(), DatabaseError<Self::Error>> {
        // "DELETE FROM books WHERE book_id = {id}"
        let idx = u64::from(id) as i64;
        execute_query!(self, "DELETE FROM books WHERE book_id = ?", idx)?;
        self.local_cache.remove_book(id);
        Ok(())
    }

    fn remove_books(&mut self, ids: &HashSet<BookID>) -> Result<(), DatabaseError<Self::Error>> {
        // "DELETE FROM books WHERE book_id IN ({ids})"
        self.local_cache.remove_books(ids);
        block_on(self.remove_books_async(ids.iter().cloned())).map_err(DatabaseError::Backend)
    }

    fn clear(&mut self) -> Result<(), DatabaseError<Self::Error>> {
        // "DELETE FROM books"
        // execute_query!(self, "DELETE FROM extended_tags")?;
        // execute_query!(self, "DELETE FROM variants")?;
        // execute_query!(self, "DELETE FROM books")?;
        block_on(self.clear_db_async()).map_err(DatabaseError::Backend)?;
        self.local_cache.clear();
        Ok(())
    }

    fn get_book(&self, id: BookID) -> Result<Arc<RwLock<Book>>, DatabaseError<Self::Error>> {
        // "SELECT * FROM books WHERE book_id = {id}"
        self.local_cache
            .get_book(id)
            .ok_or(DatabaseError::BookNotFound(id))
    }

    fn get_books<I: IntoIterator<Item = BookID>>(
        &self,
        ids: I,
    ) -> Result<Vec<Option<Arc<RwLock<Book>>>>, DatabaseError<Self::Error>> {
        // SELECT * FROM {} WHERE book_id IN ({}, );
        Ok(ids
            .into_iter()
            .map(|id| self.local_cache.get_book(id))
            .collect())
    }

    fn get_all_books(&self) -> Result<Vec<Arc<RwLock<Book>>>, DatabaseError<Self::Error>> {
        // "SELECT * FROM {} ORDER BY {} {};"
        Ok(self.local_cache.get_all_books())
    }

    fn has_column(&self, col: &UniCase<String>) -> Result<bool, DatabaseError<Self::Error>> {
        Ok(self.local_cache.has_column(col))
    }

    fn edit_book_with_id(
        &mut self,
        id: BookID,
        edits: &[(ColumnIdentifier, Edit)],
    ) -> Result<(), DatabaseError<Self::Error>> {
        // "UPDATE {} SET {} = {} WHERE book_id = {};"
        block_on(self.edit_book_by_id_async(id, edits))
    }

    fn merge_similar(&mut self) -> Result<HashSet<BookID>, DatabaseError<Self::Error>> {
        // SELECT title, book_id FROM books GROUP BY LOWER(title) HAVING COUNT(*) > 1;
        // Then, for authors ??
        let merged = self.local_cache.merge_similar_merge_ids();
        block_on(self.merge_by_ids(&merged)).map_err(DatabaseError::Backend)?;
        let to_remove = merged.into_iter().map(|(_, m)| m).collect();
        self.remove_books(&to_remove)?;
        Ok(to_remove)
    }

    fn find_matches(
        &self,
        searches: &[Search],
    ) -> Result<Vec<Arc<RwLock<Book>>>, DatabaseError<Self::Error>> {
        // "SELECT * FROM {} WHERE {} REGEXP {};"
        // FIND_MATCHES_* - look at SQLite FTS5.
        Ok(self.local_cache.find_matches(searches)?)
    }

    fn find_book_index(
        &self,
        searches: &[Search],
    ) -> Result<Option<usize>, DatabaseError<Self::Error>> {
        Ok(self.local_cache.find_book_index(searches)?)
    }

    fn sort_books_by_cols(
        &mut self,
        columns: &[(ColumnIdentifier, ColumnOrder)],
    ) -> Result<(), DatabaseError<Self::Error>> {
        self.local_cache.sort_books_by_cols(columns);
        Ok(())
    }

    fn size(&self) -> usize {
        // "SELECT COUNT(*) FROM books;"
        self.local_cache.len()
    }

    fn saved(&self) -> bool {
        true
    }

    fn update<I: IntoIterator<Item = BookVariant>>(
        &mut self,
        _books: I,
    ) -> Result<Vec<BookID>, DatabaseError<Self::Error>> {
        unimplemented!("bookstore does not currently support updating book paths.")
    }
}

impl IndexableDatabase for SQLiteDatabase {
    fn get_books_indexed(
        &self,
        indices: impl RangeBounds<usize>,
    ) -> Result<Vec<Arc<RwLock<Book>>>, DatabaseError<Self::Error>> {
        // NOTE: The query below would require a paginated search.
        //  Jumping to end is possible with the reverse search pattern,
        //  BUT: for searches, finding all results can be expensive
        // SELECT * FROM books WHERE book_id > last_id ORDER BY {} [} LIMIT {};
        let start = match indices.start_bound() {
            Bound::Included(i) => *i,
            Bound::Excluded(i) => *i + 1,
            Bound::Unbounded => 0,
        }
        .min(self.size().saturating_sub(1));

        let end = match indices.end_bound() {
            Bound::Included(i) => *i + 1,
            Bound::Excluded(i) => *i,
            Bound::Unbounded => usize::MAX,
        }
        .min(self.size());

        // let offset = start;
        // let len = end.saturating_sub(start);

        Ok((start..end)
            .filter_map(|i| self.local_cache.get_book_indexed(i))
            .collect())
    }

    fn get_book_indexed(
        &self,
        index: usize,
    ) -> Result<Arc<RwLock<Book>>, DatabaseError<Self::Error>> {
        // "SELECT * FROM {} ORDER BY {} {} LIMIT 1 OFFSET {};"
        self.local_cache
            .get_book_indexed(index)
            .ok_or(DatabaseError::IndexOutOfBounds(index))
    }

    fn remove_book_indexed(&mut self, index: usize) -> Result<(), DatabaseError<Self::Error>> {
        // "DELETE FROM books WHERE book_id > last_id ORDER BY {} {} LIMIT 1"
        // NOTE: remove_book_indexed is for removing a selected book -
        // a book can only be selected if it is already loaded - eg. in cache.
        // "DELETE FROM books WHERE book_id = {}"

        let book = self
            .local_cache
            .get_book_indexed(index)
            .ok_or(DatabaseError::IndexOutOfBounds(index))?;
        let book = book!(book);
        self.remove_book(book.id())
    }

    fn edit_book_indexed(
        &mut self,
        index: usize,
        edits: &[(ColumnIdentifier, Edit)],
    ) -> Result<(), DatabaseError<Self::Error>> {
        // "UPDATE {} SET {} = {} WHERE book_id > last_id ORDER BY {} {} LIMIT 1"
        // NOTE: edit_book_indexed is for editing a selected book -
        // a book can only be selected if it is already loaded - eg. in cache.

        let book = self
            .local_cache
            .get_book_indexed(index)
            .ok_or(DatabaseError::IndexOutOfBounds(index))?;
        let book = book!(book);
        self.edit_book_with_id(book.id(), edits)
    }
}
