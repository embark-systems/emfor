//! Reading a daemon's store the way a client does.

use std::collections::BTreeMap;
use std::path::Path;

use emfor_format::{BackendRecord, ServiceRecord};
use heed::types::Str;
use heed::{Database, Env, EnvFlags, EnvOpenOptions, WithoutTls};

/// A read-only view of a store.
pub struct Reader {
    pub env: Env<WithoutTls>,
    db: Database<Str, Str>,
}

impl Reader {
    pub fn open(path: &Path) -> Reader {
        // SAFETY: the store is opened read-only, as clients open it, and
        // nothing in the test process maps it otherwise.
        let env = unsafe {
            let mut o = EnvOpenOptions::new().read_txn_without_tls();
            o.flags(EnvFlags::NO_SUB_DIR | EnvFlags::READ_ONLY);
            o.open(path).expect("the store can be opened")
        };
        let rtxn = env.read_txn().expect("a read transaction can start");
        let db = env
            .open_database(&rtxn, None)
            .expect("the main database can be opened")
            .expect("the store has a main database");
        rtxn.commit().expect("the read transaction ends");
        Reader { env, db }
    }

    /// Every key and value.
    pub fn dump(&self) -> BTreeMap<String, String> {
        let rtxn = self.env.read_txn().expect("a read transaction can start");
        self.db
            .iter(&rtxn)
            .expect("the store can be iterated")
            .map(|r| r.expect("an entry can be read"))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    pub fn get(&self, key: &str) -> Option<String> {
        let rtxn = self.env.read_txn().expect("a read transaction can start");
        self.db
            .get(&rtxn, key)
            .expect("a key can be read")
            .map(str::to_owned)
    }

    /// The record of service `name`.
    pub fn service(&self, name: &str) -> Option<ServiceRecord> {
        self.get(&format!("svc/{name}"))
            .map(|j| serde_json::from_str(&j).expect("a service record is valid JSON"))
    }

    /// The first backend of service `name`.
    pub fn backend(&self, name: &str) -> Option<BackendRecord> {
        self.service(name).map(|s| s.backends[0].clone())
    }

    /// The `svc/` keys, in order.
    pub fn svc_keys(&self) -> Vec<String> {
        self.dump()
            .into_keys()
            .filter(|k| k.starts_with("svc/"))
            .collect()
    }
}
