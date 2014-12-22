use libc::{c_uint, c_void, size_t};
use std::{mem, ptr, raw};
use std::kinds::marker;
use std::io::BufWriter;

use ffi;

use cursor::{RoCursor, RwCursor};
use environment::Environment;
use database::Database;
use error::{LmdbError, LmdbResult, lmdb_result};
use flags::{DatabaseFlags, EnvironmentFlags, WriteFlags};

/// An LMDB transaction.
///
/// All database operations require a transaction.
pub trait Transaction<'env> {

    /// Returns a raw pointer to the underlying LMDB transaction.
    ///
    /// The caller **must** ensure that the pointer is not used after the lifetime of the
    /// transaction.
    fn txn(&self) -> *mut ffi::MDB_txn;
}

/// Transaction extension methods.
pub trait TransactionExt<'env> : Transaction<'env> {

    /// Commits the transaction.
    ///
    /// Any pending operations will be saved.
    fn commit(self) -> LmdbResult<()> {
        unsafe {
            let result = lmdb_result(ffi::mdb_txn_commit(self.txn()));
            mem::forget(self);
            result
        }
    }

    /// Aborts the transaction.
    ///
    /// Any pending operations will not be saved.
    fn abort(self) {
        // Abort should be performed in transaction destructors.
    }

    /// Gets an item from a database.
    ///
    /// This function retrieves the data associated with the given key in the database. If the
    /// database supports duplicate keys (`DatabaseFlags::DUP_SORT`) then the first data item for
    /// the key will be returned. Retrieval of other items requires the use of
    /// `Transaction::cursor_get`. If the item is not in the database, then `LmdbError::NotFound`
    /// will be returned.
    fn get<'txn>(&'txn self, database: Database, key: &[u8]) -> LmdbResult<&'txn [u8]> {
        let mut key_val: ffi::MDB_val = ffi::MDB_val { mv_size: key.len() as size_t,
                                                       mv_data: key.as_ptr() as *mut c_void };
        let mut data_val: ffi::MDB_val = ffi::MDB_val { mv_size: 0,
                                                        mv_data: ptr::null_mut() };
        unsafe {
            match ffi::mdb_get(self.txn(), database.dbi(), &mut key_val, &mut data_val) {
                ffi::MDB_SUCCESS => {
                    Ok(mem::transmute(raw::Slice {
                        data: data_val.mv_data as *const u8,
                        len: data_val.mv_size as uint
                    }))
                },
                err_code => Err(LmdbError::from_err_code(err_code)),
            }
        }
    }

    /// Open a new read-only cursor on the given database.
    fn open_read_cursor<'txn>(&'txn self, db: Database) -> LmdbResult<RoCursor<'txn>> {
        RoCursor::new(self, db)
    }

    /// Gets the option flags for the given database in the transaction.
    fn db_flags(&self, db: Database) -> LmdbResult<DatabaseFlags> {
        let mut flags: c_uint = 0;
        unsafe {
            try!(lmdb_result(ffi::mdb_dbi_flags(self.txn(), db.dbi(), &mut flags)));
        }
        Ok(DatabaseFlags::from_bits_truncate(flags))
    }
}

impl<'env, T> TransactionExt<'env> for T where T: Transaction<'env> {}

/// An LMDB read-only transaction.
pub struct RoTransaction<'env> {
    txn: *mut ffi::MDB_txn,
    _no_sync: marker::NoSync,
    _no_send: marker::NoSend,
    _contravariant: marker::ContravariantLifetime<'env>,
}

#[unsafe_destructor]
impl <'env> Drop for RoTransaction<'env> {
    fn drop(&mut self) {
        unsafe { ffi::mdb_txn_abort(self.txn) }
    }
}

impl <'env> RoTransaction<'env> {

    /// Creates a new read-only transaction in the given environment. Prefer using
    /// `Environment::begin_ro_txn`.
    #[doc(hidden)]
    pub fn new(env: &'env Environment) -> LmdbResult<RoTransaction<'env>> {
        let mut txn: *mut ffi::MDB_txn = ptr::null_mut();
        unsafe {
            try!(lmdb_result(ffi::mdb_txn_begin(env.env(),
                                                ptr::null_mut(),
                                                ffi::MDB_RDONLY,
                                                &mut txn)));
            Ok(RoTransaction {
                txn: txn,
                _no_sync: marker::NoSync,
                _no_send: marker::NoSend,
                _contravariant: marker::ContravariantLifetime::<'env>,
            })
        }
    }

    pub fn reset(self) -> InactiveTransaction<'env> {
        let txn = self.txn;
        unsafe {
            mem::forget(self);
            ffi::mdb_txn_reset(txn)
        };
        InactiveTransaction {
            txn: txn,
            _no_sync: marker::NoSync,
            _no_send: marker::NoSend,
            _contravariant: marker::ContravariantLifetime::<'env>,
        }

    }
}

impl <'env> Transaction<'env> for RoTransaction<'env> {
    fn txn(&self) -> *mut ffi::MDB_txn {
        self.txn
    }
}

/// An inactive read-only transaction.
pub struct InactiveTransaction<'env> {
    txn: *mut ffi::MDB_txn,
    _no_sync: marker::NoSync,
    _no_send: marker::NoSend,
    _contravariant: marker::ContravariantLifetime<'env>,
}

#[unsafe_destructor]
impl <'env> Drop for InactiveTransaction<'env> {
    fn drop(&mut self) {
        unsafe { ffi::mdb_txn_abort(self.txn) }
    }
}

impl <'env> InactiveTransaction<'env> {

    /// Renews the inactive transaction and returns the active read-only transaction.
    pub fn renew(self) -> LmdbResult<RoTransaction<'env>> {
        let txn = self.txn;
        unsafe {
            mem::forget(self);
            try!(lmdb_result(ffi::mdb_txn_renew(txn)))
        };
        Ok(RoTransaction {
            txn: txn,
            _no_sync: marker::NoSync,
            _no_send: marker::NoSend,
            _contravariant: marker::ContravariantLifetime::<'env>,
        })
    }
}

/// An LMDB read-write transaction.
pub struct RwTransaction<'env> {
    txn: *mut ffi::MDB_txn,
    _no_sync: marker::NoSync,
    _no_send: marker::NoSend,
    _contravariant: marker::ContravariantLifetime<'env>,
}

#[unsafe_destructor]
impl <'env> Drop for RwTransaction<'env> {
    fn drop(&mut self) {
        unsafe { ffi::mdb_txn_abort(self.txn) }
    }
}

impl <'env> RwTransaction<'env> {

    /// Creates a new read-write transaction in the given environment. Prefer using
    /// `Environment::begin_ro_txn`.
    #[doc(hidden)]
    pub fn new(env: &'env Environment) -> LmdbResult<RwTransaction<'env>> {
        let mut txn: *mut ffi::MDB_txn = ptr::null_mut();
        unsafe {
            try!(lmdb_result(ffi::mdb_txn_begin(env.env(),
                                                ptr::null_mut(),
                                                EnvironmentFlags::empty().bits(),
                                                &mut txn)));
            Ok(RwTransaction {
                txn: txn,
                _no_sync: marker::NoSync,
                _no_send: marker::NoSend,
                _contravariant: marker::ContravariantLifetime::<'env>,
            })
        }
    }

    /// Open a new read-write cursor on the given database.
    pub fn open_write_cursor<'txn>(&'txn mut self, db: Database) -> LmdbResult<RwCursor<'txn>> {
        RwCursor::new(self, db)
    }

    /// Stores an item into a database.
    ///
    /// This function stores key/data pairs in the database. The default behavior is to enter the
    /// new key/data pair, replacing any previously existing key if duplicates are disallowed, or
    /// adding a duplicate data item if duplicates are allowed (`DatabaseFlags::DUP_SORT`).
    pub fn put(&mut self,
           database: Database,
           key: &[u8],
           data: &[u8],
           flags: WriteFlags)
           -> LmdbResult<()> {
        let mut key_val: ffi::MDB_val = ffi::MDB_val { mv_size: key.len() as size_t,
                                                       mv_data: key.as_ptr() as *mut c_void };
        let mut data_val: ffi::MDB_val = ffi::MDB_val { mv_size: data.len() as size_t,
                                                        mv_data: data.as_ptr() as *mut c_void };
        unsafe {
            lmdb_result(ffi::mdb_put(self.txn(),
                                     database.dbi(),
                                     &mut key_val,
                                     &mut data_val,
                                     flags.bits()))
        }
    }

    /// Returns a `BufWriter` which can be used to write a value into the item at the given key
    /// and with the given length. The buffer must be completely filled by the caller.
    pub fn reserve<'txn>(&'txn mut self,
                     database: Database,
                     key: &[u8],
                     len: size_t,
                     flags: WriteFlags)
                     -> LmdbResult<BufWriter<'txn>> {
        let mut key_val: ffi::MDB_val = ffi::MDB_val { mv_size: key.len() as size_t,
                                                       mv_data: key.as_ptr() as *mut c_void };
        let mut data_val: ffi::MDB_val = ffi::MDB_val { mv_size: len,
                                                        mv_data: ptr::null_mut::<c_void>() };
        unsafe {
            try!(lmdb_result(ffi::mdb_put(self.txn(),
                                          database.dbi(),
                                          &mut key_val,
                                          &mut data_val,
                                          flags.bits() | ffi::MDB_RESERVE)));
            let slice: &'txn mut [u8] =
                mem::transmute(raw::Slice {
                    data: data_val.mv_data as *const u8,
                    len: data_val.mv_size as uint
                });
            Ok(BufWriter::new(slice))
        }
    }

    /// Deletes an item from a database.
    ///
    /// This function removes key/data pairs from the database. If the database does not support
    /// sorted duplicate data items (`DatabaseFlags::DUP_SORT`) the data parameter is ignored.
    /// If the database supports sorted duplicates and the data parameter is `None`, all of the
    /// duplicate data items for the key will be deleted. Otherwise, if the data parameter is
    /// `Some` only the matching data item will be deleted. This function will return
    /// `LmdbError::NotFound` if the specified key/data pair is not in the database.
    pub fn del(&mut self,
           database: Database,
           key: &[u8],
           data: Option<&[u8]>)
           -> LmdbResult<()> {
        let mut key_val: ffi::MDB_val = ffi::MDB_val { mv_size: key.len() as size_t,
                                                       mv_data: key.as_ptr() as *mut c_void };
        let data_val: Option<ffi::MDB_val> =
            data.map(|data| ffi::MDB_val { mv_size: data.len() as size_t,
                                           mv_data: data.as_ptr() as *mut c_void });
        unsafe {
            lmdb_result(ffi::mdb_del(self.txn(),
                                     database.dbi(),
                                     &mut key_val,
                                     data_val.map(|mut data_val| &mut data_val as *mut _)
                                             .unwrap_or(ptr::null_mut())))
        }
    }

    /// Begins a new nested transaction inside of this transaction.
    pub fn begin_nested_txn<'txn>(&'txn mut self) -> LmdbResult<RwTransaction<'txn>> {
        let mut nested: *mut ffi::MDB_txn = ptr::null_mut();
        unsafe {
            let env: *mut ffi::MDB_env = ffi::mdb_txn_env(self.txn());
            ffi::mdb_txn_begin(env, self.txn(), 0, &mut nested);
        }
        Ok(RwTransaction {
            txn: nested,
            _no_sync: marker::NoSync,
            _no_send: marker::NoSend,
            _contravariant: marker::ContravariantLifetime::<'env>,
        })
    }
}

impl <'env> Transaction<'env> for RwTransaction<'env> {
    fn txn(&self) -> *mut ffi::MDB_txn {
        self.txn
    }
}

#[cfg(test)]
mod test {

    use std::io;
    use std::ptr;
    use std::rand::{Rng, XorShiftRng};
    use std::sync::{Arc, Barrier, Future};
    use test::{Bencher, black_box};

    use ffi::*;

    use environment::*;
    use error::*;
    use flags::*;
    use super::*;
    use test_utils::*;

    #[test]
    fn test_put_get_del() {
        let dir = io::TempDir::new("test").unwrap();
        let env = Environment::new().open(dir.path(), io::USER_RWX).unwrap();
        let db = env.open_db(None).unwrap();

        let mut txn = env.begin_write_txn().unwrap();
        txn.put(db, b"key1", b"val1", WriteFlags::empty()).unwrap();
        txn.put(db, b"key2", b"val2", WriteFlags::empty()).unwrap();
        txn.put(db, b"key3", b"val3", WriteFlags::empty()).unwrap();
        txn.commit().unwrap();

        let mut txn = env.begin_write_txn().unwrap();
        assert_eq!(b"val1", txn.get(db, b"key1").unwrap());
        assert_eq!(b"val2", txn.get(db, b"key2").unwrap());
        assert_eq!(b"val3", txn.get(db, b"key3").unwrap());
        assert_eq!(txn.get(db, b"key"), Err(LmdbError::NotFound));

        txn.del(db, b"key1", None).unwrap();
        assert_eq!(txn.get(db, b"key1"), Err(LmdbError::NotFound));
    }

    #[test]
    fn test_reserve() {
        let dir = io::TempDir::new("test").unwrap();
        let env = Environment::new().open(dir.path(), io::USER_RWX).unwrap();
        let db = env.open_db(None).unwrap();

        let mut txn = env.begin_write_txn().unwrap();
        {
            let mut writer = txn.reserve(db, b"key1", 4, WriteFlags::empty()).unwrap();
            writer.write(b"val1").unwrap();
        }
        txn.commit().unwrap();

        let mut txn = env.begin_write_txn().unwrap();
        assert_eq!(b"val1", txn.get(db, b"key1").unwrap());
        assert_eq!(txn.get(db, b"key"), Err(LmdbError::NotFound));

        txn.del(db, b"key1", None).unwrap();
        assert_eq!(txn.get(db, b"key1"), Err(LmdbError::NotFound));
    }

    #[test]
    fn test_close_database() {
        let dir = io::TempDir::new("test").unwrap();
        let mut env = Environment::new().set_max_dbs(10)
                                        .open(dir.path(), io::USER_RWX)
                                        .unwrap();

        env.create_db(Some("db"), DatabaseFlags::empty()).unwrap();
        env.close_db(Some("db")).unwrap();
        assert!(env.open_db(Some("db")).is_ok());
    }

    #[test]
    fn test_clear_db() {
        let dir = io::TempDir::new("test").unwrap();
        let mut env = Environment::new().open(dir.path(), io::USER_RWX).unwrap();

        {
            let db = env.open_db(None).unwrap();
            let mut txn = env.begin_write_txn().unwrap();
            txn.put(db, b"key", b"val", WriteFlags::empty()).unwrap();
            txn.commit().unwrap();
        }

        env.clear_db(None).unwrap();

        let db = env.open_db(None).unwrap();
        let txn = env.begin_read_txn().unwrap();
        txn.get(db, b"key").is_err();
    }

    #[test]
    fn test_inactive_txn() {
        let dir = io::TempDir::new("test").unwrap();
        let env = Environment::new().open(dir.path(), io::USER_RWX).unwrap();
        let db = env.open_db(None).unwrap();

        {
            let mut txn = env.begin_write_txn().unwrap();
            txn.put(db, b"key", b"val", WriteFlags::empty()).unwrap();
            txn.commit().unwrap();
        }

        let txn = env.begin_read_txn().unwrap();
        let inactive = txn.reset();
        let active = inactive.renew().unwrap();
        assert!(active.get(db, b"key").is_ok());
    }

    #[test]
    fn test_nested_txn() {
        let dir = io::TempDir::new("test").unwrap();
        let env = Environment::new().open(dir.path(), io::USER_RWX).unwrap();
        let db = env.open_db(None).unwrap();

        let mut txn = env.begin_write_txn().unwrap();
        txn.put(db, b"key1", b"val1", WriteFlags::empty()).unwrap();

        {
            let mut nested = txn.begin_nested_txn().unwrap();
            nested.put(db, b"key2", b"val2", WriteFlags::empty()).unwrap();
            assert_eq!(nested.get(db, b"key1").unwrap(), b"val1");
            assert_eq!(nested.get(db, b"key2").unwrap(), b"val2");
        }

        assert_eq!(txn.get(db, b"key1").unwrap(), b"val1");
        assert_eq!(txn.get(db, b"key2"), Err(LmdbError::NotFound));
    }

    #[test]
    fn test_concurrent_readers_single_writer() {
        let dir = io::TempDir::new("test").unwrap();
        let env: Arc<Environment> = Arc::new(Environment::new().open(dir.path(), io::USER_RWX).unwrap());

        let n = 10u; // Number of concurrent readers
        let barrier = Arc::new(Barrier::new(n + 1));
        let mut futures: Vec<Future<bool>> = Vec::with_capacity(n);

        let key = b"key";
        let val = b"val";

        for _ in range(0, n) {
            let reader_env = env.clone();
            let reader_barrier = barrier.clone();

            futures.push(Future::spawn(move|| {
                let db = reader_env.open_db(None).unwrap();
                {
                    let txn = reader_env.begin_read_txn().unwrap();
                    assert_eq!(txn.get(db, key), Err(LmdbError::NotFound));
                    txn.abort();
                }
                reader_barrier.wait();
                reader_barrier.wait();
                {
                    let txn = reader_env.begin_read_txn().unwrap();
                    txn.get(db, key).unwrap() == val
                }
            }));
        }

        let db = env.open_db(None).unwrap();
        let mut txn = env.begin_write_txn().unwrap();
        barrier.wait();
        txn.put(db, key, val, WriteFlags::empty()).unwrap();
        txn.commit().unwrap();
        barrier.wait();

        assert!(futures.iter_mut().all(|b| b.get()))
    }

    #[test]
    fn test_concurrent_writers() {
        let dir = io::TempDir::new("test").unwrap();
        let env = Arc::new(Environment::new().open(dir.path(), io::USER_RWX).unwrap());

        let n = 10u; // Number of concurrent writers
        let mut futures: Vec<Future<bool>> = Vec::with_capacity(n);

        let key = "key";
        let val = "val";

        for i in range(0, n) {
            let writer_env = env.clone();

            futures.push(Future::spawn(move|| {
                let db = writer_env.open_db(None).unwrap();
                let mut txn = writer_env.begin_write_txn().unwrap();
                txn.put(db,
                        format!("{}{}", key, i).as_bytes(),
                        format!("{}{}", val, i).as_bytes(),
                        WriteFlags::empty())
                    .unwrap();
                txn.commit().is_ok()
            }));
        }
        assert!(futures.iter_mut().all(|b| b.get()));

        let db = env.open_db(None).unwrap();
        let txn = env.begin_read_txn().unwrap();

        for i in range(0, n) {
            assert_eq!(
                format!("{}{}", val, i).as_bytes(),
                txn.get(db, format!("{}{}", key, i).as_bytes()).unwrap());
        }
    }

    #[bench]
    fn bench_get_rand(b: &mut Bencher) {
        let n = 100u32;
        let (_dir, env) = setup_bench_db(n);
        let db = env.open_db(None).unwrap();
        let txn = env.begin_read_txn().unwrap();

        let mut keys: Vec<String> = range(0, n).map(|n| get_key(n))
                                               .collect::<Vec<_>>();
        XorShiftRng::new_unseeded().shuffle(keys.as_mut_slice());

        b.iter(|| {
            let mut i = 0u;
            for key in keys.iter() {
                i = i + txn.get(db, key.as_bytes()).unwrap().len();
            }
            black_box(i);
        });
    }

    #[bench]
    fn bench_get_rand_raw(b: &mut Bencher) {
        let n = 100u32;
        let (_dir, env) = setup_bench_db(n);
        let db = env.open_db(None).unwrap();
        let _txn = env.begin_read_txn().unwrap();

        let mut keys: Vec<String> = range(0, n).map(|n| get_key(n))
                                               .collect::<Vec<_>>();
        XorShiftRng::new_unseeded().shuffle(keys.as_mut_slice());

        let dbi = db.dbi();
        let txn = _txn.txn();

        let mut key_val: MDB_val = MDB_val { mv_size: 0, mv_data: ptr::null_mut() };
        let mut data_val: MDB_val = MDB_val { mv_size: 0, mv_data: ptr::null_mut() };

        b.iter(|| unsafe {
            let mut i = 0u64;
            for key in keys.iter() {
                key_val.mv_size = key.len() as u64;
                key_val.mv_data = key.as_bytes().as_ptr() as *mut _;

                mdb_get(txn, dbi, &mut key_val, &mut data_val);

                i = i + key_val.mv_size;
            }
            black_box(i);
        });
    }

    #[bench]
    fn bench_put_rand(b: &mut Bencher) {
        let n = 100u32;
        let (_dir, env) = setup_bench_db(0);
        let db = env.open_db(None).unwrap();

        let mut items: Vec<(String, String)> = range(0, n).map(|n| (get_key(n), get_data(n)))
                                                          .collect::<Vec<_>>();
        XorShiftRng::new_unseeded().shuffle(items.as_mut_slice());

        b.iter(|| {
            let mut txn = env.begin_write_txn().unwrap();
            for &(ref key, ref data) in items.iter() {
                txn.put(db, key.as_bytes(), data.as_bytes(), WriteFlags::empty()).unwrap();
            }
            txn.abort();
        });
    }

    #[bench]
    fn bench_put_rand_raw(b: &mut Bencher) {
        let n = 100u32;
        let (_dir, _env) = setup_bench_db(0);
        let db = _env.open_db(None).unwrap();

        let mut items: Vec<(String, String)> = range(0, n).map(|n| (get_key(n), get_data(n)))
                                                          .collect::<Vec<_>>();
        XorShiftRng::new_unseeded().shuffle(items.as_mut_slice());

        let dbi = db.dbi();
        let env = _env.env();

        let mut key_val: MDB_val = MDB_val { mv_size: 0, mv_data: ptr::null_mut() };
        let mut data_val: MDB_val = MDB_val { mv_size: 0, mv_data: ptr::null_mut() };

        b.iter(|| unsafe {
            let mut txn: *mut MDB_txn = ptr::null_mut();
            mdb_txn_begin(env, ptr::null_mut(), 0, &mut txn);

            let mut i = 0i32;
            for &(ref key, ref data) in items.iter() {

                key_val.mv_size = key.len() as u64;
                key_val.mv_data = key.as_bytes().as_ptr() as *mut _;
                data_val.mv_size = data.len() as u64;
                data_val.mv_data = data.as_bytes().as_ptr() as *mut _;

                i += mdb_put(txn, dbi, &mut key_val, &mut data_val, 0);
            }
            assert_eq!(0, i);
            mdb_txn_abort(txn);
        });
    }
}
