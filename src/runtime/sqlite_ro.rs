//! sqlite_ro — READ-ONLY SQLite row access (AXSEM_W2_STORAGE_RETROFIT_V1).
//!
//! One foreign fn: `sqlite_ro_tsv(db_path, sql, arg) -> Text`. It is a DUMP
//! PRODUCER: it executes a caller-authored SELECT and returns the rows as
//! TAB-separated, LF-terminated text — the exact shape `fs_read_text` returns
//! for a TSV snapshot file, so the M1 storage adapter can swap backings
//! without anything above it changing. It decides nothing: which rows, which
//! columns, and their order are entirely the caller's SQL.
//!
//! The read-only wall is enforced twice, independent of the SQL text:
//!   1. the connection is opened SQLITE_OPEN_READONLY (no CREATE) — the
//!      engine refuses any mutation of the database file;
//!   2. after prepare, `sqlite3_stmt_readonly()` must be true and the SQL
//!      must be a SINGLE statement — anything else is refused.
//! There is deliberately NO open/step/finalize handle surface and NO write,
//! exec, DDL or DML entry point in this module.
//!
//! The engine stays the system libsqlite3 (`#[link(name = "sqlite3")]`);
//! nothing SQL-shaped is reimplemented here.

use super::value::{Value, intern_str, get_str};
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};

#[allow(non_camel_case_types)]
type sqlite3 = c_void;
#[allow(non_camel_case_types)]
type sqlite3_stmt = c_void;

const SQLITE_OPEN_READONLY: c_int = 0x0000_0001;
const SQLITE_OK: c_int = 0;
const SQLITE_ROW: c_int = 100;
const SQLITE_DONE: c_int = 101;
/// SQLITE_TRANSIENT — sqlite copies the bound text before returning.
const TRANSIENT: isize = -1;

#[link(name = "sqlite3")]
extern "C" {
    fn sqlite3_open_v2(filename: *const c_char, db: *mut *mut sqlite3,
                       flags: c_int, vfs: *const c_char) -> c_int;
    fn sqlite3_close(db: *mut sqlite3) -> c_int;
    fn sqlite3_errmsg(db: *mut sqlite3) -> *const c_char;
    fn sqlite3_busy_timeout(db: *mut sqlite3, ms: c_int) -> c_int;
    fn sqlite3_prepare_v2(db: *mut sqlite3, sql: *const c_char, n: c_int,
                          stmt: *mut *mut sqlite3_stmt, tail: *mut *const c_char) -> c_int;
    fn sqlite3_stmt_readonly(stmt: *mut sqlite3_stmt) -> c_int;
    fn sqlite3_bind_parameter_count(stmt: *mut sqlite3_stmt) -> c_int;
    fn sqlite3_bind_text(stmt: *mut sqlite3_stmt, idx: c_int, text: *const c_char,
                         n: c_int, destructor: isize) -> c_int;
    fn sqlite3_step(stmt: *mut sqlite3_stmt) -> c_int;
    fn sqlite3_column_count(stmt: *mut sqlite3_stmt) -> c_int;
    fn sqlite3_column_text(stmt: *mut sqlite3_stmt, col: c_int) -> *const u8;
    fn sqlite3_column_bytes(stmt: *mut sqlite3_stmt, col: c_int) -> c_int;
    fn sqlite3_finalize(stmt: *mut sqlite3_stmt) -> c_int;
}

fn errmsg(db: *mut sqlite3) -> String {
    unsafe {
        let p = sqlite3_errmsg(db);
        if p.is_null() { "unknown sqlite error".into() }
        else { CStr::from_ptr(p).to_string_lossy().into_owned() }
    }
}

/// RAII guard so the connection (and statement) are released on every exit
/// path, including the panic paths — a long-running serve process that
/// catches the unwind must not leak read locks on the live database.
struct Conn(*mut sqlite3);
impl Drop for Conn {
    fn drop(&mut self) { unsafe { sqlite3_close(self.0); } }
}
struct Stmt(*mut sqlite3_stmt);
impl Drop for Stmt {
    fn drop(&mut self) { unsafe { sqlite3_finalize(self.0); } }
}

/// `sqlite_ro_tsv(db_path: Text, sql: Text, arg: Text) -> Text`
///
/// Runs one read-only statement and returns its rows as TSV text (columns
/// TAB-joined, NULL as empty, one LF after every row). `arg` is bound to ?1
/// when the statement declares a parameter; pass "" otherwise.
#[track_caller]
pub fn sqlite_ro_tsv(args: Value) -> Value {
    let (path, sql_text, arg_text) = match args {
        Value::Tuple(ref es) if es.len() >= 3 => {
            let p = match &es[0] { Value::Str(h) => get_str(h), _ => panic!("sqlite_ro_tsv: expected Str db_path") };
            let s = match &es[1] { Value::Str(h) => get_str(h), _ => panic!("sqlite_ro_tsv: expected Str sql") };
            let a = match &es[2] { Value::Str(h) => get_str(h), _ => panic!("sqlite_ro_tsv: expected Str arg") };
            (p, s, a)
        }
        _ => panic!("sqlite_ro_tsv: expected Tuple(Text, Text, Text)"),
    };

    let c_path = CString::new(path.clone())
        .unwrap_or_else(|_| panic!("sqlite_ro_tsv({}): NUL in path", path));
    let c_sql = CString::new(sql_text)
        .unwrap_or_else(|_| panic!("sqlite_ro_tsv({}): NUL in sql", path));

    unsafe {
        let mut raw_db: *mut sqlite3 = std::ptr::null_mut();
        let rc = sqlite3_open_v2(c_path.as_ptr(), &mut raw_db,
                                 SQLITE_OPEN_READONLY, std::ptr::null());
        let db = Conn(raw_db); // close even when open failed (per sqlite docs)
        if rc != SQLITE_OK {
            panic!("sqlite_ro_tsv({}): open read-only failed: {}", path, errmsg(db.0));
        }
        // Reads retry briefly instead of failing if the (rollback-journal)
        // writer holds the lock for a moment. Read-side patience only.
        sqlite3_busy_timeout(db.0, 2000);

        let mut raw_stmt: *mut sqlite3_stmt = std::ptr::null_mut();
        let mut tail: *const c_char = std::ptr::null();
        let rc = sqlite3_prepare_v2(db.0, c_sql.as_ptr(), -1, &mut raw_stmt, &mut tail);
        if rc != SQLITE_OK {
            panic!("sqlite_ro_tsv({}): prepare failed: {}", path, errmsg(db.0));
        }
        let stmt = Stmt(raw_stmt);
        if stmt.0.is_null() {
            panic!("sqlite_ro_tsv({}): empty sql", path);
        }
        if !tail.is_null() && !CStr::from_ptr(tail).to_bytes().iter()
                .all(|b| b.is_ascii_whitespace()) {
            panic!("sqlite_ro_tsv({}): multiple statements refused", path);
        }
        if sqlite3_stmt_readonly(stmt.0) == 0 {
            panic!("sqlite_ro_tsv({}): non-read-only statement refused", path);
        }

        if sqlite3_bind_parameter_count(stmt.0) >= 1 {
            let rc = sqlite3_bind_text(stmt.0, 1, arg_text.as_ptr() as *const c_char,
                                       arg_text.len() as c_int, TRANSIENT);
            if rc != SQLITE_OK {
                panic!("sqlite_ro_tsv({}): bind failed: {}", path, errmsg(db.0));
            }
        }

        let mut out = String::new();
        loop {
            match sqlite3_step(stmt.0) {
                SQLITE_ROW => {
                    let ncol = sqlite3_column_count(stmt.0);
                    for col in 0..ncol {
                        if col > 0 { out.push('\t'); }
                        let p = sqlite3_column_text(stmt.0, col);
                        if !p.is_null() {
                            let n = sqlite3_column_bytes(stmt.0, col) as usize;
                            let bytes = std::slice::from_raw_parts(p, n);
                            out.push_str(&String::from_utf8_lossy(bytes));
                        }
                    }
                    out.push('\n');
                }
                SQLITE_DONE => break,
                _ => panic!("sqlite_ro_tsv({}): step failed: {}", path, errmsg(db.0)),
            }
        }
        Value::Str(intern_str(&out))
    }
}
