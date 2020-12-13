use std::collections::hash_map::RandomState;
use std::collections::HashSet;
use std::ops::Range;
use std::path;
use std::path::Path;

use sqlx::{Connection, Sqlite, SqliteConnection};
use unicase::UniCase;

use crate::database::search::Search;
use crate::database::{AppDatabase, DatabaseError, IndexableDatabase};
use crate::record::Book;
use sqlx::migrate::MigrateDatabase;

const CREATE_BOOKS: &str = r#"CREATE TABLE `books` (
`book_id` INTEGER PRIMARY KEY UNIQUE,
`title` TEXT DEFAULT NULL,
`series_name` TEXT DEFAULT NULL,
`series_id` NUM DEFAULT NULL
);"#;

// Authors are stored here as well.
const CREATE_EXTENDED_TAGS: &str = r#"CREATE TABLE `extended_tags` (
`tag_name` TEXT,
`tag_value` TEXT,
`book_id` INTEGER NOT NULL,
FOREIGN KEY(book_id) REFERENCES books(book_id)
    ON UPDATE CASCADE
    ON DELETE CASCADE
);"#;

const CREATE_VARIANTS: &str = r#"CREATE TABLE `variants` (
`book_type` TEXT,
`path` TEXT,
`local_title` TEXT DEFAULT NULL,
`identifier` TEXT DEFAULT NULL,
`language` TEXT DEFAULT NULL,
`description` TEXT DEFAULT NULL,
`id` INTEGER DEFAULT NULL,
`book_id` INTEGER NOT NULL,
FOREIGN KEY(book_id) REFERENCES books(book_id)
    ON UPDATE CASCADE
    ON DELETE CASCADE
);"#;

struct SQLiteDatabase {
    backend: SqliteConnection,
    /// All available columns. Case-insensitive.
    cols: HashSet<UniCase<String>>,
    len: usize,
    saved: bool,
}

impl AppDatabase for SQLiteDatabase {
    fn open<P>(file_path: P) -> Result<Self, DatabaseError>
    where
        P: AsRef<path::Path>,
        Self: Sized,
    {
        let path = file_path.as_ref().display().to_string();
        if !Sqlite::database_exists(path.as_str()).await.unwrap() {
            Sqlite::create_database(path.as_str()).await.unwrap();
        }
        let database = SqliteConnection::connect(path.as_str()).await.unwrap();
        Ok(Self {
            backend: database,
            cols: Default::default(),
            len: 0,
            saved: false,
        })
    }

    fn save(&mut self) -> Result<(), DatabaseError> {
        unimplemented!()
    }

    fn insert_book(&mut self, book: Book) -> Result<u32, DatabaseError> {
        unimplemented!()
    }

    fn read_book_from_file<P>(&mut self, file_path: P) -> Result<u32, DatabaseError>
    where
        P: AsRef<path::Path>,
    {
        unimplemented!()
    }

    fn read_books_from_dir<P>(
        &mut self,
        dir: P,
    ) -> Result<(Vec<u32>, Vec<DatabaseError>), DatabaseError>
    where
        P: AsRef<path::Path>,
    {
        unimplemented!()
    }

    fn remove_book(&mut self, id: u32) -> Result<(), DatabaseError> {
        unimplemented!()
    }

    fn remove_books(&mut self, ids: Vec<u32>) -> Result<(), DatabaseError> {
        unimplemented!()
    }

    fn clear(&mut self) -> Result<(), DatabaseError> {
        unimplemented!()
    }

    fn get_book(&self, id: u32) -> Result<Book, DatabaseError> {
        unimplemented!()
    }

    fn get_books(&self, ids: Vec<u32>) -> Result<Vec<Option<Book>>, DatabaseError> {
        unimplemented!()
    }

    fn get_all_books(&self) -> Result<Vec<Book>, DatabaseError> {
        unimplemented!()
    }

    fn get_available_columns(&self) -> &HashSet<UniCase<String>, RandomState> {
        unimplemented!()
    }

    fn has_column(&self, col: &UniCase<String>) -> bool {
        unimplemented!()
    }

    fn edit_book_with_id<S0: AsRef<str>, S1: AsRef<str>>(
        &mut self,
        id: u32,
        column: S0,
        new_value: S1,
    ) -> Result<(), DatabaseError> {
        unimplemented!()
    }

    fn merge_similar(&mut self) -> Result<(), DatabaseError> {
        unimplemented!()
    }

    fn find_matches(&self, search: Search) -> Result<Vec<Book>, DatabaseError> {
        unimplemented!()
    }

    fn sort_books_by_col(&mut self, col: &str, reverse: bool) -> Result<(), DatabaseError> {
        unimplemented!()
    }

    fn size(&self) -> usize {
        unimplemented!()
    }

    fn saved(&self) -> bool {
        unimplemented!()
    }
}

impl IndexableDatabase for SQLiteDatabase {
    fn get_books_indexed(&self, indices: Range<usize>) -> Result<Vec<Book>, DatabaseError> {
        unimplemented!()
    }

    fn get_book_indexed(&self, index: usize) -> Result<Book, DatabaseError> {
        unimplemented!()
    }

    fn remove_book_indexed(&mut self, index: usize) -> Result<(), DatabaseError> {
        unimplemented!()
    }

    fn edit_book_indexed<S0: AsRef<str>, S1: AsRef<str>>(
        &mut self,
        index: usize,
        column: S0,
        new_value: S1,
    ) -> Result<(), DatabaseError> {
        unimplemented!()
    }
}