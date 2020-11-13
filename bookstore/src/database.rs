use std::collections::{HashMap, HashSet};

use rustbreak::{deser::Ron, FileDatabase, RustbreakError};
use serde::{Deserialize, Serialize};

use std::fs;

use crate::book::{Book, BookError};
use std::path;

#[derive(Debug)]
pub(crate) enum DatabaseError {
    IoError(std::io::Error),
    BookReadingError(BookError),
    BackendError(RustbreakError),
}

impl From<std::io::Error> for DatabaseError {
    fn from(e: std::io::Error) -> Self {
        DatabaseError::IoError(e)
    }
}

impl From<RustbreakError> for DatabaseError {
    fn from(e: RustbreakError) -> Self {
        DatabaseError::BackendError(e)
    }
}

impl From<BookError> for DatabaseError {
    fn from(e: BookError) -> Self {
        DatabaseError::BookReadingError(e)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub(crate) struct BookMap {
    books: HashMap<u32, Book>,
}

pub(crate) struct BasicDatabase {
    backend: FileDatabase<BookMap, Ron>,
}

pub(crate) trait AppDatabase {
    /// Opens the database at the path if it exists.
    ///
    /// # Arguments
    ///
    /// * ` file_path ` - A path to a database.
    ///
    /// # Errors
    /// This function will return an error if the file can not be found, or the database
    /// is itself invalid.
    fn open<S>(file_path: S) -> Result<Self, DatabaseError>
    where
        S: AsRef<path::Path>,
        Self: Sized;

    /// Saves the database to its original location.
    ///
    /// # Errors
    /// This function will return an error if the database can not be saved correctly.
    fn save(&self) -> Result<(), DatabaseError>;

    /// Reads each book in the directory into the database, and returns a
    /// Vec of corresponding IDs.
    ///
    /// # Arguments
    /// * ` dir ` - A path to directories containing books to load.
    ///
    /// # Errors
    /// This function will return an error if the database fails,
    /// the directory does not exist, or reading a file fails.
    fn read_books_from_dir<S>(&self, dir: S) -> Result<Vec<u32>, DatabaseError>
    where
        S: AsRef<path::Path>;

    /// Reads the book at the given location into the database, and returns the book's ID.
    ///
    /// # Arguments
    /// * ` file_path ` - The path to the book to be read.
    ///
    /// # Errors
    /// This function will return an error if the database fails,
    /// the file does not exist, or can not be read.
    fn read_book_from_file<S>(&self, file_path: S) -> Result<u32, DatabaseError>
    where
        S: AsRef<path::Path>;

    /// Inserts the given book into the database. If the book does not have an ID, it is given
    /// an ID equal to the largest ID in the database so far, plus one.
    ///
    /// # Arguments
    /// * ` book ` - A book to be stored.
    ///
    /// # Errors
    /// This function will return an error if the database fails.
    fn insert_book(&self, book: Book) -> Result<u32, DatabaseError>;

    /// Removes the book with the given ID. If no book with the given ID exists, no change occurs.
    ///
    /// # Arguments
    /// * ` id ` - The ID of the book to be removed.
    ///
    /// # Errors
    /// This function will return an error if the database fails.
    fn remove_book(&self, id: u32) -> Result<(), DatabaseError>;

    /// Returns a copy of every book in the database. If reading fails, None is returned.
    fn get_all_books(&self) -> Option<Vec<Book>>;

    /// Finds and returns the book with the given ID. If no book is found, nothing is returned.
    ///
    /// # Arguments
    /// * ` id ` - The ID of the book to be returned.
    fn get_book(&self, id: u32) -> Option<Book>;

    /// Finds and returns the books with the given IDs, and if a particular book is not found,
    /// nothing is returned for that particular book.
    fn get_books(&self, ids: Vec<u32>) -> Vec<Option<Book>>;

    /// Returns a list of columns that exist for at least one book in the database.
    fn get_available_columns(&self) -> Option<Vec<String>>;
}

impl AppDatabase for BasicDatabase {
    fn open<S>(file_path: S) -> Result<Self, DatabaseError>
    where
        S: AsRef<path::Path>,
    {
        Ok(BasicDatabase {
            backend: FileDatabase::<BookMap, Ron>::load_from_path_or_default(file_path)?,
        })
    }

    fn save(&self) -> Result<(), DatabaseError> {
        Ok(self.backend.save()?)
    }

    fn read_books_from_dir<S>(&self, dir: S) -> Result<Vec<u32>, DatabaseError>
    where
        S: AsRef<path::Path>,
    {
        fs::read_dir(dir)?
            .map(|res| res.map(|e| e.path()))
            .collect::<Result<Vec<_>, std::io::Error>>()?
            .iter()
            .map(|path| self.read_book_from_file(path))
            .collect::<Result<Vec<_>, _>>()
    }

    fn read_book_from_file<S>(&self, file_path: S) -> Result<u32, DatabaseError>
    where
        S: AsRef<path::Path>,
    {
        match self.insert_book(Book::generate_from_file(file_path)?) {
            Ok(id) => Ok(id),
            Err(e) => Err(DatabaseError::from(e)),
        }
    }

    fn insert_book(&self, book: Book) -> Result<u32, DatabaseError> {
        Ok(self.backend.write(|db| {
            if let Some(id) = book.get_id() {
                db.books.insert(id, book);
                id
            } else {
                let index = db.books.keys().max().unwrap_or(&0) + 1;
                let mut book = book.clone();
                book.set_id(index);
                db.books.insert(index, book);
                index
            }
        })?)
    }

    fn remove_book(&self, id: u32) -> Result<(), DatabaseError> {
        Ok(self.backend.write(|db| {
            db.books.remove(&id);
        })?)
    }

    fn get_all_books(&self) -> Option<Vec<Book>> {
        match self.backend.read(|db| -> Result<Option<Vec<Book>>, ()> {
            Ok(Some(
                db.books.values().into_iter().map(|b| b.clone()).collect(),
            ))
        }) {
            Ok(book) => book.unwrap_or(None),
            Err(_) => None,
        }
    }

    fn get_book(&self, id: u32) -> Option<Book> {
        match self.backend.read(|db| match db.books.get(&id) {
            None => Err(()),
            Some(book) => Ok(Some(book.clone())),
        }) {
            Ok(book) => book.unwrap_or(None),
            Err(_) => None,
        }
    }

    fn get_books(&self, ids: Vec<u32>) -> Vec<Option<Book>> {
        ids.iter().map(|&id| self.get_book(id)).collect()
    }

    fn get_available_columns(&self) -> Option<Vec<String>> {
        match self
            .backend
            .read(|db| -> Result<Option<HashSet<String>>, ()> {
                let mut c = HashSet::new();
                c.insert("title".to_string());
                c.insert("authors".to_string());
                c.insert("series".to_string());
                c.insert("id".to_string());
                for book in db.books.values() {
                    if let Some(e) = book.get_extended_columns() {
                        for key in e.keys() {
                            c.insert(key.clone());
                        }
                    }
                }
                Ok(Some(c))
            }) {
            Ok(book) => {
                if let Ok(x) = book {
                    if let Some(hset) = x {
                        return Some(hset.iter().map(|col| col.clone()).collect());
                    }
                }
                None
            }
            Err(_) => None,
        }
    }
}