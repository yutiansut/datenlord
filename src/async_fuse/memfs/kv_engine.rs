use super::{s3_node::S3Node, s3_wrapper::S3BackEnd, INum, S3MetaData};
use crate::common::async_fuse_error::KVEngineError;
use crate::common::error::{Context, DatenLordError, DatenLordResult};
use async_trait::async_trait;
use clippy_utilities::OverflowArithmetic;
use core::fmt::Debug;
use etcd_client::{Compare, CompareOp, DeleteOptions, LockOptions, PutOptions, Txn, TxnOp};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Display;
use std::sync::Arc;
use std::{fmt, time::Duration};

/// The `KVEngineType` is used to provide support for metadata.
/// We use this alias to avoid tem
pub type KVEngineType = EtcdKVEngine;

use std::sync::Arc;

use super::serial::{SerialDirEntry, SerialFileAttr, SerialNode};

/// The `ValueType` is used to provide support for metadata.
#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
pub enum ValueType {
    /// SerialNode
    Node(SerialNode),
    /// SerialDirEntry
    DirEntry(SerialDirEntry),
    /// INum
    INum(INum),
    /// FileAttr
    Attr(SerialFileAttr),
    /// Next id allocate range begin
    NextIdAllocateRangeBegin(u64),
    /// Raw value
    Raw(Vec<u8>),
    /// String value
    String(String),
}

impl ValueType {
    #[allow(dead_code, clippy::wildcard_enum_match_arm)] // Allow wildcard because there should be only one enum branch matches one specific type.
    /// Turn the `ValueType` into `SerialNode` then into `S3Node`.
    /// # Panics
    /// Panics if `ValueType` is not `ValueType::Node`.
    pub async fn into_s3_node<S: S3BackEnd + Send + Sync + 'static>(
        self,
        meta: &S3MetaData<S>,
    ) -> S3Node<S> {
        match self {
            ValueType::Node(node) => S3Node::from_serial_node(node, meta).await,
            _ => {
                panic!("expect ValueType::Node but get {self:?}");
            }
        }
    }

    /// Turn the `ValueType` into `NextIdAllocateRangeBegin`.
    /// # Panics
    /// Panics if `ValueType` is not `ValueType::NextIdAllocateRangeBegin`.
    #[allow(clippy::wildcard_enum_match_arm)] // Allow wildcard because there should be only one enum branch matches one specific type.
    #[must_use]
    pub fn into_next_id_allocate_range_begin(self) -> u64 {
        match self {
            ValueType::NextIdAllocateRangeBegin(begin) => begin,
            _ => panic!("expect ValueType::NextIdAllocateRangeBegin but get {self:?}"),
        }
    }

    /// Turn the `ValueType` into `INum`.
    /// # Panics
    /// Panics if `ValueType` is not `ValueType::INum`.
    #[allow(clippy::wildcard_enum_match_arm)] // Allow wildcard because there should be only one enum branch matches one specific type.
    #[must_use]
    pub fn into_inum(self) -> INum {
        match self {
            ValueType::INum(inum) => inum,
            _ => panic!("expect ValueType::INum but get {self:?}"),
        }
    }

    /// Turn the `ValueType` into `String`
    /// # Panics
    /// Panics if `ValueType` is not `ValueType::String`.
    #[allow(clippy::wildcard_enum_match_arm)] // Allow wildcard because there should be only one enum branch matches one specific type.
    #[must_use]
    pub fn into_string(self) -> String {
        match self {
            ValueType::String(string) => string,
            _ => panic!("expect ValueType::String but get {self:?}"),
        }
    }

    /// Turn the `ValueType` into `Raw`
    /// # Panics
    /// Panics if `ValueType` is not `ValueType::Raw`.
    #[allow(clippy::wildcard_enum_match_arm)] // Allow wildcard because there should be only one enum branch matches one specific type.
    #[must_use]
    pub fn into_raw(self) -> Vec<u8> {
        match self {
            ValueType::Raw(raw) => raw,
            _ => panic!("expect ValueType::Raw but get {self:?}"),
        }
    }
}

/// The `KeyType` is used to locate the value in the distributed K/V storage.
/// Every key is prefixed with a string to indicate the type of the value.
/// If you want to add a new type of value, you need to add a new variant to the enum.
/// And you need to add a new match arm to the `get_key` function , make sure the key is unique.
#[allow(dead_code)]
#[derive(Debug, Eq, PartialEq)]
pub enum KeyType {
    /// INum -> SerailNode
    INum2Node(INum),
    /// INum -> DirEntry
    INum2DirEntry(INum),
    /// Path -> Inum
    Path2INum(String),
    /// INum -> SerialFileAttr
    INum2Attr(INum),
    /// IdAllocator value key
    IdAllocatorValue {
        /// the prefix of the key for different id type
        unique_id: u8,
    },
    /// Csi related key
    Csi(String),
    /// ETCD node ip and port info : node_id -> "{node_ipaddr}:{port}"
    /// The corresponding value type is ValueType::String
    NodeIpPort(String),
    /// ETCD volume information
    /// The corresponding value type is ValueType::RawData
    VolumeInfo(String),
    /// ETCD file node list
    /// The corresponding value type is ValueType::RawData
    FileNodeList(Vec<u8>),
}

// ::<KeyType>::get() -> ValueType
/// Lock key type the memfs used.
#[derive(Debug, Eq, PartialEq)]
#[allow(variant_size_differences)]
pub enum LockKeyType {
    /// IdAllocator lock key
    IdAllocatorLock {
        /// the prefix of the key for different id type
        unique_id: u8,
    },
    /// ETCD volume information lock
    VolumeInfoLock,
    /// ETCD file node list lock
    FileNodeListLock(Vec<u8>),
}

impl Display for KeyType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            KeyType::INum2Node(ref i) => write!(f, "INum2Node{{i: {i}}}"),
            KeyType::INum2DirEntry(ref i) => write!(f, "INum2DirEntry{{i: {i}}}"),
            KeyType::Path2INum(ref p) => write!(f, "Path2INum{{p: {p}}}"),
            KeyType::INum2Attr(ref i) => write!(f, "INum2Attr{{i: {i}}}"),
            KeyType::IdAllocatorValue { unique_id } => {
                write!(f, "IdAllocatorValue{{unique_id: {unique_id}}}")
            }
            KeyType::Csi(ref s) => write!(f, "Csi{{s: {s}}}"),
            KeyType::NodeIpPort(ref s) => write!(f, "NodeIpPort{{s: {s}}}"),
            KeyType::VolumeInfo(ref s) => write!(f, "VolumeInfo{{s: {s}}}"),
            KeyType::FileNodeList(ref s) => write!(f, "FileNodeList{{s: {s:?}}}"),
        }
    }
}

impl Display for LockKeyType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            LockKeyType::IdAllocatorLock { unique_id } => {
                write!(f, "IdAllocatorLock{{unique_id: {unique_id}}}")
            }
            LockKeyType::VolumeInfoLock => {
                write!(f, "LockKeyType::VolumeInfoLock ")
            }
            LockKeyType::FileNodeListLock(ref file_name) => {
                write!(f, "LockKeyType::FileNodeList {{file_name: {file_name:?}}}")
            }
        }
    }
}

/// Get key serialized data with prefix and key.
#[inline]
fn serialize_key<K: ?Sized + Serialize>(key_prefix: u16, key: &K) -> Vec<u8> {
    let mut v = vec![];
    bincode::serialize_into(&mut v, &key_prefix).unwrap_or_else(|e| {
        panic!("serialize key prefix failed, err:{e}");
    });
    assert_eq!(v.len(), 2);
    bincode::serialize_into(&mut v, key).unwrap_or_else(|e| {
        panic!("serialize key failed, err:{e}");
    });

    v
}

impl KeyType {
    /// Get the key in bytes.
    #[must_use]
    pub fn get_key(&self) -> Vec<u8> {
        match *self {
            KeyType::INum2Node(ref i) => serialize_key(0, i),
            KeyType::INum2DirEntry(ref i) => serialize_key(1, i),
            KeyType::Path2INum(ref p) => serialize_key(2, p),
            KeyType::INum2Attr(ref i) => serialize_key(3, i),
            KeyType::IdAllocatorValue { unique_id } => serialize_key(4, &unique_id),
            KeyType::Csi(ref s) => serialize_key(5, s),
            KeyType::NodeIpPort(ref s) => serialize_key(6, s),
            KeyType::VolumeInfo(ref s) => serialize_key(8, s),
            KeyType::FileNodeList(ref s) => serialize_key(10, s),
        }
    }
}

impl LockKeyType {
    /// Get the key in vec bytes.
    fn get_key(&self) -> Vec<u8> {
        match *self {
            LockKeyType::IdAllocatorLock { unique_id } => serialize_key(100, &unique_id),
            LockKeyType::VolumeInfoLock => serialize_key(101, &0_i32),
            LockKeyType::FileNodeListLock(ref file_name) => serialize_key(102, file_name),
        }
    }
}

/// The Txn is used to provide support for metadata.
#[async_trait]
pub trait MetaTxn {
    /// Get the value by the key.
    /// Notice : do not get the same key twice in one transaction.
    #[must_use]
    async fn get(&mut self, key: &KeyType) -> DatenLordResult<Option<ValueType>>;
    /// Set the value by the key.
    fn set(&mut self, key: &KeyType, value: &ValueType);
    /// Delete the value by the key.
    fn delete(&mut self, key: &KeyType);
    /// Commit the transaction.
    /// Only when commit is called, the write operations will be executed.
    /// If the commit is successful, return true, else return false.
    async fn commit(&mut self) -> DatenLordResult<bool>;
}

/// The option of 'set' operation
/// Currently support 'lease' and `prev_kv`
/// `lease` is used to set the lease of the key
/// `prev_kv` is used to return the previous key-value pair
#[allow(dead_code)]
#[derive(Debug, Eq, PartialEq)]
pub struct SetOption {
    /// The lease of the key
    pub(crate) lease: Option<i64>,
    /// Whether to return the previous key-value pair
    pub(crate) prev_kv: bool,
}

impl SetOption {
    #[allow(dead_code)]
    #[must_use]
    /// Create a new `SetOption`
    /// Default lease is None, `prev_kv` is false
    fn new() -> Self {
        Self {
            lease: None,
            prev_kv: false,
        }
    }

    #[allow(dead_code)]
    #[must_use]
    /// Set the lease of the key
    fn with_lease(mut self, lease: i64) -> Self {
        self.lease = Some(lease);
        self
    }

    #[allow(dead_code)]
    #[must_use]
    /// Set whether to return the previous key-value pair
    fn with_prev_kv(mut self) -> Self {
        self.prev_kv = true;
        self
    }
}

/// The option of 'delete' operation
/// Currently support `prev_kv` and `range_end`
/// `prev_kv` is used to return the previous key-value pair
/// `range_end` is used to delete all keys in the range [key, `range_end`)
#[allow(dead_code)]
#[derive(Debug, Eq, PartialEq)]
pub struct DeleteOption {
    /// Whether to return the previous key-value pair
    pub(crate) prev_kv: bool,
    /// The range end of the delete operation
    pub(crate) range_end: Option<Vec<u8>>,
}

impl DeleteOption {
    #[allow(dead_code)]
    #[must_use]
    /// Create a new `DeleteOption`
    fn new() -> Self {
        Self {
            prev_kv: false,
            range_end: None,
        }
    }

    #[allow(dead_code)]
    #[must_use]
    /// Set whether to return the previous key-value pair
    fn with_prev_kv(mut self) -> Self {
        self.prev_kv = true;
        self
    }

    #[allow(dead_code)]
    #[must_use]
    /// Set the range end of the delete operation
    fn with_range_end(mut self, range_end: Vec<u8>) -> Self {
        self.range_end = Some(range_end);
        self
    }

    /// Delete all keys
    #[allow(dead_code)]
    #[must_use]
    /// Set the range end of the delete operation
    fn with_all_keys(mut self) -> Self {
        self.range_end = Some(vec![0xff]);
        self
    }
}

#[allow(dead_code)]
#[derive(Debug, Eq, PartialEq)]
/// The option of 'lock' operation
/// Currently, only the lease is supported.
pub struct LockOption {
    /// The lease of the lock
    pub(crate) lease: Option<i64>,
}

impl Default for LockOption {
    fn default() -> Self {
        Self::new()
    }
}

impl LockOption {
    #[allow(dead_code)]
    #[must_use]
    /// Create a new lock option without lease
    pub fn new() -> Self {
        Self { lease: None }
    }

    #[allow(dead_code)]
    #[must_use]
    /// Change the lease of the lock option
    pub fn with_lease(mut self, lease: i64) -> Self {
        self.lease = Some(lease);
        self
    }
}

/// `KeyRange` is an abstraction for describing etcd key of various types.
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Debug)]
#[allow(dead_code)]
pub struct KeyRange {
    /// The first key of the range and should be non-empty
    pub(crate) key: Vec<u8>,
    /// The key following the last key of the range
    pub(crate) range_end: Vec<u8>,
}

impl KeyRange {
    /// Creates a new `KeyRange` for describing a range of multiple keys.
    #[inline]
    #[allow(dead_code)]
    pub fn range<K, R>(key: K, range_end: R) -> Self
    where
        K: Into<Vec<u8>>,
        R: Into<Vec<u8>>,
    {
        Self {
            key: key.into(),
            range_end: range_end.into(),
        }
    }

    /// Creates a new `KeyRange` for describing a specified key.
    #[inline]
    #[allow(dead_code)]
    pub fn key<K>(key: K) -> Self
    where
        K: Into<Vec<u8>>,
    {
        Self {
            key: key.into(),
            range_end: vec![],
        }
    }

    /// Creates a new `KeyRange` for describing all keys.
    #[inline]
    #[must_use]
    #[allow(dead_code)]
    pub fn all() -> Self {
        Self {
            key: vec![0],
            range_end: vec![0],
        }
    }

    /// Creates a new `KeyRange` for describing keys prefixed with specified value.
    #[inline]
    #[allow(dead_code)]
    pub fn prefix<K>(prefix: K) -> Self
    where
        K: Into<Vec<u8>>,
    {
        let key = prefix.into();
        if key.is_empty() {
            // An empty Vec<u8> results in an invalid KeyRange.
            // Assume that an empty value passed to this method implies no prefix (i.e., all keys).
            return Self::all();
        }

        let mut first_value = true;
        let mut range_end = key
            .iter()
            .rev()
            .filter_map(|e| {
                if *e < 0xff {
                    if first_value {
                        first_value = false;
                        Some(e.overflow_add(1))
                    } else {
                        Some(*e)
                    }
                } else {
                    None
                }
            })
            .collect::<Vec<u8>>();
        range_end.reverse();
        Self { key, range_end }
    }

    /// Take key value
    #[inline]
    #[allow(dead_code)]
    pub(crate) fn take_key(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.key)
    }

    /// Take `range_end` value
    #[inline]
    #[allow(dead_code)]
    pub(crate) fn take_range_end(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.range_end)
    }
}

// impl the From trait for KeyRange to etcd_client::KeyRange
impl From<KeyRange> for etcd_client::KeyRange {
    fn from(mut key_range: KeyRange) -> Self {
        etcd_client::KeyRange::range(
            std::mem::take(&mut key_range.key),
            std::mem::take(&mut key_range.range_end),
        )
    }
}

/// To support different K/V storage engines, we need to a trait to abstract the K/V storage engine.
#[async_trait]
pub trait KVEngine: Send + Sync + Debug + Sized {
    /// create a new KVEngine.
    async fn new(end_points: Vec<String>) -> DatenLordResult<Self>;
    /// Create a new transaction.
    async fn new_meta_txn(&self) -> Box<dyn MetaTxn + Send>;
    /// Distribute lock - lock
    async fn lock(
        &self,
        key: &LockKeyType,
        timeout: Duration,
        option: Option<LockOption>,
    ) -> DatenLordResult<Vec<u8>>;
    /// Distribute lock - unlock
    async fn unlock(&self, key: &LockKeyType) -> DatenLordResult<()>;
    /// Get the value by the key.
    async fn get(&self, key: &KeyType) -> DatenLordResult<Option<ValueType>>;
    /// Set the value by the key.
    async fn set(
        &self,
        key: &KeyType,
        value: &ValueType,
        option: Option<SetOption>,
    ) -> DatenLordResult<Option<ValueType>>;
    /// Delete the kv pair by the key.
    async fn delete(
        &self,
        key: &KeyType,
        option: Option<DeleteOption>,
    ) -> DatenLordResult<Option<ValueType>>;

    /// Lease grant
    async fn lease_grant(&self, ttl: i64) -> DatenLordResult<i64>;

    /// Range query
    async fn range(&self, key_range: KeyRange) -> DatenLordResult<Vec<(Vec<u8>, Vec<u8>)>>;
}

/// The version of the key.
type KvVersion = i64;

/// Convert u64 seceond to i64
fn conv_u64_sec_2_i64(sec: u64) -> i64 {
    sec.try_into()
        .unwrap_or_else(|e| panic!("ttl timeout_sec should < MAX_I64, err:{e}"))
}

/// Fix ttl, ttl should be > 0
fn check_ttl(sec: i64) -> DatenLordResult<i64> {
    if sec <= 0 {
        Err(DatenLordError::KVEngineErr {
            source: KVEngineError::WrongTimeoutArg,
            context: vec!["Timeout arg for kv should be >= 1 second".to_owned()],
        })
    } else {
        Ok(sec)
    }
}

/// The version of the key.
type KvVersion = i64;

/// Convert u64 seceond to i64
fn conv_u64_sec_2_i64(sec: u64) -> i64 {
    sec.try_into()
        .unwrap_or_else(|e| panic!("ttl timeout_sec should < MAX_I64, err:{e}"))
}

/// Fix ttl, ttl should be > 0
fn check_ttl(sec: i64) -> DatenLordResult<i64> {
    if sec <= 0 {
        Err(DatenLordError::KVEngineErr {
            source: KVEngineError::WrongTimeoutArg,
            context: vec!["Timeout arg for kv should be >= 1 second".to_owned()],
        })
    } else {
        Ok(sec)
    }
}

/// The version of the key.
type KvVersion = i64;

/// Convert u64 seceond to i64
fn conv_u64_sec_2_i64(sec: u64) -> i64 {
    sec.try_into()
        .unwrap_or_else(|e| panic!("ttl timeout_sec should < MAX_I64, err:{e}"))
}

/// Fix ttl, ttl should be > 0
fn check_ttl(sec: i64) -> DatenLordResult<i64> {
    if sec <= 0 {
        Err(DatenLordError::KVEngineErr {
            source: KVEngineError::WrongTimeoutArg,
            context: vec!["Timeout arg for kv should be >= 1 second".to_owned()],
        })
    } else {
        Ok(sec)
    }
}

/// The version of the key.
type KvVersion = i64;

/// Convert u64 seceond to i64
fn conv_u64_sec_2_i64(sec: u64) -> i64 {
    sec.try_into()
        .unwrap_or_else(|e| panic!("ttl timeout_sec should < MAX_I64, err:{e}"))
}

/// Fix ttl, ttl should be > 0
fn check_ttl(sec: i64) -> DatenLordResult<i64> {
    if sec <= 0 {
        Err(DatenLordError::KVEngineErr {
            source: KVEngineError::WrongTimeoutArg,
            context: vec!["Timeout arg for kv should be >= 1 second".to_owned()],
        })
    } else {
        Ok(sec)
    }
}

/// The version of the key.
type KvVersion = i64;

/// Convert u64 seceond to i64
fn conv_u64_sec_2_i64(sec: u64) -> i64 {
    sec.try_into()
        .unwrap_or_else(|e| panic!("ttl timeout_sec should < MAX_I64, err:{e}"))
}

/// Fix ttl, ttl should be > 0
fn check_ttl(sec: i64) -> DatenLordResult<i64> {
    if sec <= 0 {
        Err(DatenLordError::KVEngineErr {
            source: KVEngineError::WrongTimeoutArg,
            context: vec!["Timeout arg for kv should be >= 1 second".to_owned()],
        })
    } else {
        Ok(sec)
    }
}

/// The version of the key.
type KvVersion = i64;

/// Convert u64 seceond to i64
fn conv_u64_sec_2_i64(sec: u64) -> i64 {
    sec.try_into()
        .unwrap_or_else(|e| panic!("ttl timeout_sec should < MAX_I64, err:{e}"))
}

/// Fix ttl, ttl should be > 0
fn check_ttl(sec: i64) -> DatenLordResult<i64> {
    if sec <= 0 {
        Err(DatenLordError::KVEngineErr {
            source: KVEngineError::WrongTimeoutArg,
            context: vec!["Timeout arg for kv should be >= 1 second".to_owned()],
        })
    } else {
        Ok(sec)
    }
}

/// The version of the key.
type KvVersion = i64;

/// Convert u64 seceond to i64
fn conv_u64_sec_2_i64(sec: u64) -> i64 {
    sec.try_into()
        .unwrap_or_else(|e| panic!("ttl timeout_sec should < MAX_I64, err:{e}"))
}

/// Fix ttl, ttl should be > 0
fn check_ttl(sec: i64) -> DatenLordResult<i64> {
    if sec <= 0 {
        Err(DatenLordError::KVEngineErr {
            source: KVEngineError::WrongTimeoutArg,
            context: vec!["Timeout arg for kv should be >= 1 second".to_owned()],
        })
    } else {
        Ok(sec)
    }
}

/// The version of the key.
type KvVersion = i64;

/// Convert u64 seceond to i64
fn conv_u64_sec_2_i64(sec: u64) -> i64 {
    sec.try_into()
        .unwrap_or_else(|e| panic!("ttl timeout_sec should < MAX_I64, err:{e}"))
}

/// Fix ttl, ttl should be > 0
fn check_ttl(sec: i64) -> DatenLordResult<i64> {
    if sec <= 0 {
        Err(DatenLordError::KVEngineErr {
            source: KVEngineError::WrongTimeoutArg,
            context: vec!["Timeout arg for kv should be >= 1 second".to_owned()],
        })
    } else {
        Ok(sec)
    }
}

/// The `etcd`'s transaction impl.
/// The txn won't do anything until commit is called.
/// Write operations are buffered until commit is called.
struct EtcdTxn {
    /// The etcd client.
    client: etcd_client::Client,
    /// The key is the key in bytes, the value is the version of the key.
    version_map: HashMap<Vec<u8>, KvVersion>,
    /// Store the write operations in the buffer.
    buffer: HashMap<Vec<u8>, Option<Vec<u8>>>,
}

impl EtcdTxn {
    /// Create a new etcd transaction.
    fn new(client: etcd_client::Client) -> Self {
        EtcdTxn {
            client,
            version_map: HashMap::new(),
            buffer: HashMap::new(),
        }
    }
}

#[async_trait]
impl MetaTxn for EtcdTxn {
    async fn get(&mut self, key: &KeyType) -> DatenLordResult<Option<ValueType>> {
        // first check if the key is in buffer (write op)
        let key = key.get_key();
        assert!(
            self.buffer.get(&key).is_none(),
            "get the key after write in the same transaction"
        );
        assert!(
            self.version_map.get(&key).is_none(),
            "get the key twice in the same transaction"
        );
        // Fetch the value from `etcd`
        let resp = self
            .client
            .get(key.clone(), None)
            .await
            .with_context(|| "failed to get at `MetaTxn::get`".to_owned())?;
        let kvs = resp.kvs();
        // we don't expect to have multiple values for one key
        assert!(kvs.len() <= 1, "multiple values for one key");
        if let Some(kv) = kvs.get(0) {
            let value = kv.value();
            // update the version_map
            self.version_map.insert(key.clone(), kv.version());
            Ok(Some(serde_json::from_slice(value)?))
        } else {
            // update the version_map
            self.version_map.insert(key, 0);
            Ok(None)
        }
    }

    fn set(&mut self, key: &KeyType, value: &ValueType) {
        let key = key.get_key();
        // Because the ValueType derives the serde::Serialize
        // This unwrap will not panic.
        let value = serde_json::to_vec(value)
            .unwrap_or_else(|value| panic!("failed to serialize value to json,value = {value:?}"));
        self.buffer.insert(key, Some(value));
    }

    fn delete(&mut self, key: &KeyType) {
        let key = key.get_key();
        self.buffer.insert(key, None);
    }

    async fn commit(&mut self) -> DatenLordResult<bool> {
        if self.version_map.is_empty() && self.buffer.is_empty() {
            return Ok(true);
        }

        let resp = self
            .client
            .txn(
                Txn::new()
                    .when(
                        self.version_map
                            .iter()
                            .map(|(key, version)| {
                                Compare::version(key.clone(), CompareOp::Equal, *version)
                            })
                            .collect::<Vec<Compare>>(),
                    )
                    .and_then(
                        self.buffer
                            .iter()
                            .map(|(key, value)| {
                                if let Some(ref value) = *value {
                                    TxnOp::put(key.clone(), value.clone(), None)
                                } else {
                                    TxnOp::delete(key.clone(), None)
                                }
                            })
                            .collect::<Vec<TxnOp>>(),
                    ),
            )
            .await
            .with_context(|| "failed to do txn operation at `MetaTxn::commit`".to_owned())?;
        Ok(resp.succeeded())
    }
}

#[derive(Clone)]
/// Wrap the etcd client to support the `KVEngine` trait.
pub struct EtcdKVEngine {
    /// The etcd client.
    client: etcd_client::Client,
}

impl Debug for EtcdKVEngine {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EtcdKVEngine").finish()
    }
}

impl EtcdKVEngine {
    #[allow(dead_code)]
    /// For local test, we need to create a new etcd kv engine locally.
    async fn new_for_local_test(etcd_address_vec: Vec<String>) -> DatenLordResult<Self> {
        let client = etcd_client::Client::connect(etcd_address_vec.clone(), None)
            .await
            .with_context(|| {
                format!("failed to connect to etcd, the etcd address={etcd_address_vec:?}")
            })?;
        Ok(EtcdKVEngine { client })
    }

    #[allow(dead_code)]
    #[must_use]
    /// Create a new etcd kv engine.
    pub fn new_kv_engine(etcd_client: etcd_client::Client) -> Arc<Self> {
        Arc::new(EtcdKVEngine {
            client: etcd_client,
        })
    }
}

/// break `retry_txn` result
pub const RETRY_TXN_BREAK: DatenLordResult<bool> = Ok(true);

#[allow(unused_macros, clippy::crate_in_macro_def)]
#[macro_export]
/// the logic should return a tuple (txn commit/cancel result, res)
/// For example: `Ok((txn.commit().await,...))`
/// For example: `Ok((BREAK_RETRY_TXN,...))`
macro_rules! retry_txn {
    ($retry_num : expr ,$logic: block) => {{
        use crate::common::error::DatenLordError;

        let mut result = Err(DatenLordError::TransactionRetryLimitExceededErr {
            context: vec!["Transaction retry failed due to exceeding the retry limit".to_owned()],
        });
        let mut attempts: u32 = 0;

        while attempts < $retry_num {
            attempts = attempts.wrapping_add(1);
            let (commit_res, res) = { $logic };
            match commit_res {
                Ok(commit_res) => {
                    if commit_res {
                        result = Ok(res);
                        break;
                    }
                }
                Err(err) => {
                    result = Err(err);
                    break;
                }
            }
        }
        result
    }};
}

#[async_trait]
impl KVEngine for EtcdKVEngine {
    #[must_use]
    async fn new(end_points: Vec<String>) -> DatenLordResult<Self> {
        let client = etcd_client::Client::connect(etcd_client::ClientConfig::new(
            end_points.clone(),
            None,
            64,
            true,
        ))
        .await
        .with_context(|| format!("failed to build etcd client to addresses={end_points:?}"))?;
        Ok(EtcdKVEngine { client })
    }
    async fn new_meta_txn(&self) -> Box<dyn MetaTxn + Send> {
        Box::new(EtcdTxn::new(self.client.clone()))
    }

    async fn lease_grant(&self, ttl: i64) -> DatenLordResult<i64> {
        let req = etcd_client::EtcdLeaseGrantRequest::new(Duration::from_secs(
            ttl.try_into()
                .unwrap_or_else(|_| panic!("Unexpected i64 to u64 conversion error.")),
        ));
        let resp = self
            .client
            .lease()
            .grant(req)
            .await
            .with_context(|| "failed to get LeaseGrantResponse from etcd".to_owned())?;
        Ok(resp
            .id()
            .try_into()
            .unwrap_or_else(|_| panic!("Unexpected u64 to i64 conversion error.")))
    }

    async fn range(&self, key_range: KeyRange) -> DatenLordResult<Vec<(Vec<u8>, Vec<u8>)>> {
        let req = etcd_client::EtcdRangeRequest::new(key_range.into());
        let mut resp = self
            .client
            .kv()
            .range(req)
            .await
            .with_context(|| "failed to get RangeResponse from etcd".to_owned())?;
        let kvs = resp.take_kvs();
        let mut result = Vec::with_capacity(kvs.len());
        for kv in &kvs {
            result.push((kv.key().to_vec(), kv.value().to_vec()));
        }
        Ok(result)
    }

    /// Distribute lock - lock
    /// - `timeout_sec` should be >=1s
    /// - `timeout_sec` should be >=1s
    async fn lock(&self, key: &LockKeyType, timeout_sec: Duration) -> DatenLordResult<()> {
        let mut client = self.client.clone();
        let timeout_sec = check_ttl(conv_u64_sec_2_i64(timeout_sec.as_secs()))
            .with_context(|| "timeout_sec should be >=1s, please fix the call".to_owned())?;

        let lease_id = client
            .lease_grant(timeout_sec, None)
            .await
            .with_context(|| "failed to get lease at `MetaTxn::lock`".to_owned())?
            .id();

        let _ = client
            .lock(key.get_key(), Some(LockOptions::new().with_lease(lease_id)))
            .await
            .with_context(|| "failed to lock at `MetaTxn::lock`".to_owned())?;

        Ok(())
    }

    /// Distribute lock - unlock
    async fn unlock(&self, key: &LockKeyType) -> DatenLordResult<()> {
        let mut client = self.client.clone();
        client
            .unlock(key.get_key())
            .await
            .with_context(|| "failed to unlock at `MetaTxn::unlock`".to_owned())?;

        Ok(())
    }

    /// Get the value by the key.
    async fn get(&self, key: &KeyType) -> DatenLordResult<Option<ValueType>> {
        let mut client = self.client.clone();
        let resp = client
            .get(key.get_key(), None)
            .await
            .with_context(|| format!("failed to get at `MetaTxn::get`, key={key:?}"))?;

        let kvs = resp.kvs();
        match kvs.get(0) {
            Some(kv) => Ok(Some(serde_json::from_slice::<ValueType>(kv.value()).with_context(||{
                "failed to deserialize value from bytes, KVEngine's value supposed to be `ValueType`".to_owned()
            })?)),
            None => Ok(None),
        }
    }
    /// Set the value by the key.
    async fn set(
        &self,
        key: &KeyType,
        value: &ValueType,
        _option: Option<SetOption>,
    ) -> DatenLordResult<Option<ValueType>> {
        // TODO : add option support
        let serial_value = serde_json::to_vec(value)
            .with_context(|| format!("failed to serialize value={value:?} to bytes"))?;
        let mut client = self.client.clone();
        let mut resp = client
            .put(
                key.get_key(),
                serial_value,
                Some(PutOptions::new().with_prev_key()),
            )
            .await
            .with_context(|| "failed to put at `MetaTxn::set`".to_owned())?;
        if let Some(pre_kv) = resp.take_prev_key() {
            let decoded_value: ValueType = serde_json::from_slice(pre_kv.value())?;
            Ok(Some(decoded_value))
        } else {
            Ok(None)
        }
    }

    /// Delete the kv pair by the key.
    async fn delete(&self, key: &KeyType) -> DatenLordResult<Option<ValueType>> {
        let resp = self
            .client
            .kv_client()
            .delete(key.get_key(), Some(DeleteOptions::new().with_prev_key()))
            .await
            .with_context(|| format!("failed to get DeleteResponse from etcd for key={key:?}"))?;
        if let Some(pre_kv) = resp.prev_kvs().first() {
            let decoded_value: ValueType = serde_json::from_slice(pre_kv.value())?;
            Ok(Some(decoded_value))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod test {

    use super::*;
    use crate::common::error::DatenLordError;

    const ETCD_ADDRESS: &str = "localhost:2379";

    #[tokio::test]
    async fn test_key_serial() {
        let key = KeyType::INum2Attr(1).get_key();
        assert_eq!(key.len(), 10);
        let key = KeyType::IdAllocatorValue { unique_id: 0 }.get_key();
        assert_eq!(key.len(), 3);
    }

    #[tokio::test]
    async fn test_connect_local() {
        let client = EtcdKVEngine::new_for_local_test(vec![ETCD_ADDRESS.to_owned()])
            .await
            .unwrap();
        // insert a key , and then get it , and then delete it, and then get it again
        let key = KeyType::Path2INum("test_key".to_owned());
        let value = ValueType::INum(123);
        client.set(&key, &value, None).await.unwrap();
        let get_value = client.get(&key).await.unwrap().unwrap();
        assert_eq!(get_value, value);
        client.delete(&key, None).await.unwrap();
        let get_value = client.get(&key).await.unwrap();
        assert!(get_value.is_none());
    }

    #[tokio::test]
    async fn test_easy_commit_fail() {
        // Generate three transactions
        // The first one will set two keys and commit
        // And the second one read two keys
        // And the third one will set two keys and commit
        // What we expect is that the second one will fail
        // Between it's read ,the third one will set the same key
        let client = EtcdKVEngine::new_for_local_test(vec![ETCD_ADDRESS.to_owned()])
            .await
            .unwrap();
        let mut first_txn = client.new_meta_txn().await;
        let key1 = KeyType::Path2INum(String::from("/"));
        let value1 = ValueType::INum(12);
        let key2 = KeyType::Path2INum(String::from("/a"));
        let value2 = ValueType::INum(13);
        first_txn.set(&key1, &value1);
        first_txn.set(&key2, &value2);
        first_txn.commit().await.unwrap();
        drop(client);
        // use two thread to do the second and third txn
        // and use channel to control the order
        let (first_step_tx, mut first_step_rx) = tokio::sync::mpsc::channel(1);
        let (second_step_tx, mut second_step_rx) = tokio::sync::mpsc::channel(1);
        let second_handle = tokio::spawn(async move {
            let result = retry_txn!(1, {
                let client = EtcdKVEngine::new_for_local_test(vec![ETCD_ADDRESS.to_owned()])
                    .await
                    .unwrap();
                let mut second_txn = client.new_meta_txn().await;
                let key1 = KeyType::Path2INum(String::from("/"));
                let value1 = second_txn.get(&key1).await.unwrap();
                assert!(value1.is_some());
                if let Some(ValueType::INum(num)) = value1 {
                    assert_eq!(num, 12);
                } else {
                    panic!("wrong value type");
                }
                // let the third txn start
                first_step_tx.send(()).await.unwrap();
                // wait for the third txn to set the key
                second_step_rx.recv().await.unwrap();
                let key2 = KeyType::Path2INum(String::from("/a"));
                let value2 = second_txn.get(&key2).await.unwrap();
                assert!(value2.is_some());
                if let Some(ValueType::INum(num)) = value2 {
                    assert_eq!(num, 13);
                } else {
                    panic!("wrong value type");
                }
                (second_txn.commit().await, ())
            });
            assert!(result.is_err());
            // check if the err is TransactionRetryLimitExceededErr
            if let Err(DatenLordError::TransactionRetryLimitExceededErr { .. }) = result {
            } else {
                panic!("wrong error type");
            }
        });
        let third_handle = tokio::spawn(async move {
            let client = EtcdKVEngine::new_for_local_test(vec![ETCD_ADDRESS.to_owned()])
                .await
                .unwrap();
            let mut third_txn = client.new_meta_txn().await;
            // wait for the second read first key and send the signal
            first_step_rx.recv().await.unwrap();
            let key1 = KeyType::Path2INum(String::from("/"));
            let value1 = ValueType::INum(14);
            third_txn.set(&key1, &value1);
            third_txn.commit().await.unwrap();
            // send the signal to the second txn
            second_step_tx.send(()).await.unwrap();
        });
        second_handle.await.unwrap();
        third_handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_txn_retry() {
        let result = retry_txn!(3, {
            let client = EtcdKVEngine::new_for_local_test(vec![ETCD_ADDRESS.to_owned()])
                .await
                .unwrap();
            let mut txn = client.new_meta_txn().await;
            let key = KeyType::Path2INum(String::from("/"));
            let _ = txn.get(&key).await.unwrap();
            (txn.commit().await, ())
        });
        result.unwrap();
    }
}
