use std::path::PathBuf;
use std::str::FromStr;

use bookstore_database::search::{Search, SearchMode};
use bookstore_records::book::{BookID, ColumnIdentifier};

#[derive(Debug)]
pub enum Flag {
    /// Flag followed by another flag or nothing.
    Flag(String),
    /// Flag followed by non-flag arguments.
    FlagWithArgument(String, Vec<String>),
    /// Arguments appearing without preceeding flag.
    StartingArguments(Vec<String>),
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum BookIndex {
    Selected,
    ID(BookID),
}

#[derive(Debug, PartialEq)]
pub enum Command {
    DeleteBook(BookIndex),
    DeleteAll,
    // TODO: Add + syntax for appending to existing text
    EditBook(BookIndex, Box<[(String, String)]>),
    AddBookFromFile(PathBuf),
    AddBooksFromDir(PathBuf, u8),
    AddColumn(String),
    RemoveColumn(String),
    SortColumns(Box<[(String, bool)]>),
    OpenBookInApp(BookIndex, usize),
    OpenBookInExplorer(BookIndex, usize),
    TryMergeAllBooks,
    Quit,
    Write,
    WriteAndQuit,
    FindMatches(Box<[Search]>),
    JumpTo(Box<[Search]>),
    Help(String),
    GeneralHelp,
    // TODO:
    //  eg. :m 1 2 -> merge 2 into 1
    //      Add MergeBooks with criteria?
    //  eg. :m -c (Search)*
    //      Allow adding multiple books or directories at once?
}

#[derive(Debug)]
pub enum CommandError {
    UnknownCommand,
    InsufficientArguments,
    InvalidCommand,
}

impl Command {
    pub fn requires_ui(&self) -> bool {
        use Command::*;
        match self {
            DeleteBook(b) | EditBook(b, _) | OpenBookInApp(b, _) | OpenBookInExplorer(b, _) => {
                b == &BookIndex::Selected
            }
            AddColumn(_) | RemoveColumn(_) | SortColumns(_) | FindMatches(_) => true,
            _ => false,
        }
    }
}

/// Read a white-space split command string, and collect flags and arguments together.
/// Returns a vec of Flags corresponding to the input.
/// Flag-argument sets of the following forms are handled:
/// * -f1 arg1 arg2 ... -f2?
/// * arg1 arg2 arg3 ... -f1?
/// * -f1 -f2
///
/// Note: Does not handle -abcdef.
///
/// # Arguments
/// * ` args ` - A vector of command line arguments in sequential order.
fn read_flags(args: Vec<String>) -> Vec<Flag> {
    if args.len() <= 1 {
        return vec![];
    }

    let mut flags = vec![];
    let mut last_flag_valid = false;
    let mut flag = String::new();
    let mut flag_args = vec![];

    let mut v_iter = args.into_iter();
    v_iter.next();

    for v in v_iter {
        if v.starts_with('-') {
            if last_flag_valid {
                if flag_args.is_empty() {
                    flags.push(Flag::Flag(flag));
                } else {
                    flags.push(Flag::FlagWithArgument(flag, flag_args));
                    flag_args = vec![];
                }
            } else if !flag_args.is_empty() {
                flags.push(Flag::StartingArguments(flag_args));
                flag_args = vec![];
            }

            flag = v.trim_start_matches('-').to_owned();
            last_flag_valid = true;
        } else {
            flag_args.push(v);
        }
    }

    if last_flag_valid {
        if flag_args.is_empty() {
            flags.push(Flag::Flag(flag));
        } else {
            flags.push(Flag::FlagWithArgument(flag, flag_args));
        }
    } else {
        flags.push(Flag::StartingArguments(flag_args));
    }

    flags
}

fn remove_string_quotes(mut s: String) -> String {
    match s.chars().next() {
        x @ Some('"') | x @ Some('\'') => match s.chars().last() {
            y @ Some('"') | y @ Some('\'') => {
                if x == y {
                    s.remove(0);
                    s.pop();
                    s
                } else {
                    s
                }
            }
            _ => s,
        },
        _ => s,
    }
}

#[allow(dead_code)]
/// Parses `s` into a command, using shell-style string splitting.
///
/// # Arguments
/// * ` s ` - The command in string format.
///
/// # Errors
/// Returns an error if the string does not have a matching command, or is a malformed command.
pub fn parse_command_string<S: AsRef<str>>(s: S) -> Result<Command, CommandError> {
    match shellwords::split(s.as_ref()) {
        Ok(vec) => parse_args(vec),
        Err(_) => Err(CommandError::InvalidCommand),
    }
}

/// Reads `args` and returns the corresponding command. If no corresponding command exists,
/// an error is returned.
///
/// # Arguments
/// * ` args ` - The arguments to turn into a command.
///
/// # Errors
/// If the command is missing required arguments, or is unrecognized, an error is returned.
pub fn parse_args(args: Vec<String>) -> Result<Command, CommandError> {
    let c = if let Some(c) = args.first() {
        c.clone()
    } else {
        return Err(CommandError::InsufficientArguments);
    };

    let insuf = || CommandError::InsufficientArguments;

    let flags = read_flags(args);
    match c.as_str() {
        ":q" => Ok(Command::Quit),
        ":w" => Ok(Command::Write),
        ":wq" => Ok(Command::WriteAndQuit),
        ":a" => {
            let mut d = false;
            let mut depth = 1;
            let mut path_exists = false;
            let mut path = PathBuf::new();

            for flag in flags {
                match flag {
                    Flag::Flag(c) => match c.as_str() {
                        "r" => {
                            depth = 255;
                        }
                        "d" => {
                            d = true;
                        }
                        _ => {}
                    },
                    Flag::FlagWithArgument(c, args) => match c.as_str() {
                        "r" => match u8::from_str(&args[0]) {
                            Ok(i) => depth = i,
                            Err(_) => return Err(CommandError::InvalidCommand),
                        },
                        "d" => {
                            d = true;
                            path_exists = true;
                            path = PathBuf::from(&args[0]);
                        }
                        _ => {}
                    },
                    Flag::StartingArguments(args) => {
                        path_exists = true;
                        path = PathBuf::from(&args[0]);
                    }
                };
            }
            if path_exists {
                Ok(if d {
                    Command::AddBooksFromDir(path, depth)
                } else {
                    Command::AddBookFromFile(path)
                })
            } else {
                Err(CommandError::InsufficientArguments)
            }
        }
        ":d" => {
            if let Some(flag) = flags.first() {
                match flag {
                    Flag::Flag(a) => {
                        if a == "a" {
                            Ok(Command::DeleteAll)
                        } else {
                            Err(CommandError::InvalidCommand)
                        }
                    }
                    Flag::StartingArguments(args) => {
                        if let Ok(i) = BookID::from_str(args[0].as_str()) {
                            Ok(Command::DeleteBook(BookIndex::ID(i)))
                        } else {
                            Err(CommandError::InvalidCommand)
                        }
                    }
                    _ => Err(CommandError::InvalidCommand),
                }
            } else {
                Ok(Command::DeleteBook(BookIndex::Selected))
            }
        }
        ":e" => match flags.into_iter().next() {
            Some(Flag::StartingArguments(args)) => {
                let mut chunks = Vec::with_capacity(args.len() / 2);
                let (id, mut args) = if args.len() % 2 == 1 {
                    let mut args = args.into_iter();
                    (
                        Some(
                            BookID::from_str(&args.next().ok_or_else(insuf)?)
                                .map_err(|_| CommandError::InvalidCommand)?,
                        ),
                        args,
                    )
                } else {
                    (None, args.into_iter())
                };
                while let Some(field) = args.next() {
                    let value = args.next().ok_or_else(insuf)?;
                    chunks.push((field, value));
                }

                if chunks.is_empty() {
                    return Err(CommandError::InsufficientArguments);
                }

                if let Some(id) = id {
                    Ok(Command::EditBook(
                        BookIndex::ID(id),
                        chunks.into_boxed_slice(),
                    ))
                } else {
                    Ok(Command::EditBook(
                        BookIndex::Selected,
                        chunks.into_boxed_slice(),
                    ))
                }
            }
            _ => Err(CommandError::InvalidCommand),
        },
        ":m" => match flags.first() {
            Some(Flag::Flag(a)) => {
                if a == "a" {
                    Ok(Command::TryMergeAllBooks)
                } else {
                    Err(CommandError::InvalidCommand)
                }
            }
            _ => Err(CommandError::InvalidCommand),
        },
        ":s" => {
            let mut sort_cols = Vec::new();

            for flag in flags.into_iter() {
                match flag {
                    Flag::FlagWithArgument(f, args) => {
                        if f != "d" {
                            return Err(CommandError::InvalidCommand);
                        }
                        let mut args = args.into_iter();
                        sort_cols.push((args.next().ok_or_else(insuf)?, true));
                        sort_cols.extend(args.map(|s| (s, false)));
                    }
                    Flag::StartingArguments(args) => {
                        sort_cols.extend(args.into_iter().map(|s| (s, false)));
                    }
                    _ => {
                        return Err(CommandError::InvalidCommand);
                    }
                };
            }

            if sort_cols.is_empty() {
                Err(CommandError::InsufficientArguments)
            } else {
                Ok(Command::SortColumns(sort_cols.into_boxed_slice()))
            }
        }
        ":c" => match flags.into_iter().next() {
            Some(Flag::StartingArguments(args)) => Ok(Command::AddColumn(
                args.into_iter().next().ok_or_else(insuf)?,
            )),
            Some(Flag::Flag(arg)) => Ok(Command::RemoveColumn(arg)),
            _ => Err(CommandError::InvalidCommand),
        },
        x @ ":f" | x @ ":j" => {
            let mut matches = vec![];
            for flag in flags.into_iter() {
                let (mode, args) = match flag {
                    Flag::StartingArguments(args) => Ok((SearchMode::Default, args)),
                    Flag::FlagWithArgument(flag, args) => match flag.as_str() {
                        "r" => Ok((SearchMode::Regex, args)),
                        "e" => Ok((SearchMode::ExactSubstring, args)),
                        "x" => Ok((SearchMode::ExactString, args)),
                        _ => Err(CommandError::InvalidCommand),
                    },
                    _ => Err(CommandError::InvalidCommand),
                }?;
                let mut args = args.into_iter();
                matches.push(Search {
                    mode,
                    column: ColumnIdentifier::from(args.next().ok_or_else(insuf)?),
                    search: remove_string_quotes(args.next().ok_or_else(insuf)?),
                });
                while let Some(col) = args.next() {
                    let search = args.next().ok_or_else(insuf)?;
                    matches.push(Search {
                        mode: SearchMode::Default,
                        column: ColumnIdentifier::from(col),
                        search: remove_string_quotes(search),
                    });
                }
            }

            if matches.is_empty() {
                Err(CommandError::InsufficientArguments)
            } else if x == ":f" {
                Ok(Command::FindMatches(matches.into_boxed_slice()))
            } else if x == ":j" {
                Ok(Command::JumpTo(matches.into_boxed_slice()))
            } else {
                unreachable!("x can only be one of :f, :j (is {})", x);
            }
        }
        ":o" => {
            let mut f = false;
            let mut loc_exists = false;
            let mut loc = String::new();
            let mut index_exists = false;
            let mut index = String::new();
            for flag in flags {
                match flag {
                    Flag::Flag(c) => {
                        f |= c == "f";
                    }
                    Flag::FlagWithArgument(c, args) => {
                        if c == "f" {
                            if let Some(ind_book) = args.get(0) {
                                if let Ok(bi) = BookID::from_str(ind_book.as_str()) {
                                    if let Some(ind_var) = args.get(1) {
                                        if let Ok(vi) = usize::from_str(ind_var.as_str()) {
                                            return Ok(Command::OpenBookInExplorer(
                                                BookIndex::ID(bi),
                                                vi,
                                            ));
                                        }
                                    }
                                    return Ok(Command::OpenBookInExplorer(BookIndex::ID(bi), 0));
                                }
                            }
                        }
                        return Err(CommandError::InvalidCommand);
                    }
                    Flag::StartingArguments(args) => {
                        let mut args = args.into_iter();

                        if let Some(l) = args.next() {
                            loc_exists = true;
                            loc = l;
                        }

                        if let Some(i) = args.next() {
                            index_exists = true;
                            index = i;
                        }
                    }
                }
            }
            if loc_exists {
                if let Ok(loc) = BookID::from_str(loc.as_str()) {
                    if index_exists {
                        if let Ok(index) = usize::from_str(index.as_str()) {
                            return Ok(if f {
                                Command::OpenBookInExplorer(BookIndex::ID(loc), index)
                            } else {
                                Command::OpenBookInApp(BookIndex::ID(loc), index)
                            });
                        }
                    } else if f {
                        return Ok(Command::OpenBookInExplorer(BookIndex::ID(loc), 0));
                    } else {
                        return Ok(Command::OpenBookInApp(BookIndex::ID(loc), 0));
                    }
                }
            }
            Ok(if f {
                Command::OpenBookInExplorer(BookIndex::Selected, 0)
            } else {
                Command::OpenBookInApp(BookIndex::Selected, 0)
            })
        }
        ":h" => Ok(match flags.into_iter().next() {
            Some(Flag::StartingArguments(args)) => Command::Help(
                args.into_iter()
                    .next()
                    .ok_or(CommandError::InsufficientArguments)?,
            ),
            _ => Command::GeneralHelp,
        }),
        _ => Err(CommandError::UnknownCommand),
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_add_command() {
        let args = vec![
            (
                vec![":a", "hello world", "-d"],
                Command::AddBooksFromDir(PathBuf::from("hello world"), 1),
            ),
            (
                vec![":a", "-d", "hello world"],
                Command::AddBooksFromDir(PathBuf::from("hello world"), 1),
            ),
            (
                vec![":a", "-d", "hello world", "-r", "1"],
                Command::AddBooksFromDir(PathBuf::from("hello world"), 1),
            ),
            (
                vec![":a", "-r", "1", "-d", "hello world"],
                Command::AddBooksFromDir(PathBuf::from("hello world"), 1),
            ),
            (
                vec![":a", "-d", "hello world", "-r"],
                Command::AddBooksFromDir(PathBuf::from("hello world"), 255),
            ),
            (
                vec![":a", "-r", "-d", "hello world"],
                Command::AddBooksFromDir(PathBuf::from("hello world"), 255),
            ),
            (
                vec![":a", "hello world"],
                Command::AddBookFromFile(PathBuf::from("hello world")),
            ),
        ];

        for (args, command) in args {
            let args: Vec<_> = args.into_iter().map(|s| s.to_owned()).collect();
            let res = parse_args(args.clone()).unwrap();
            assert_eq!(res, command, "from {:?} expected {:?}", args, command);
        }
    }
}
