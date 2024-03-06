use si_layer_cache::disk_cache::DiskCache;

#[test]
fn new() {
    let tempdir = tempfile::tempdir().expect("cannot create tempdir");
    let db = sled::open(tempdir).expect("unable to open sled database");
    let _disk_cache: DiskCache<&[u8]> =
        DiskCache::new(db, "random?").expect("cannot create disk cache and a tree for each type");
}

#[tokio::test]
async fn insert_and_get() {
    let tempdir = tempfile::tempdir().expect("cannot create tempdir");
    let db = sled::open(tempdir).expect("unable to open sled database");
    let disk_cache = DiskCache::new(db, "insert_and_get")
        .expect("cannot create disk cache and a tree for each type");
    disk_cache
        .insert(b"skid row", b"slave to the grind")
        .expect("cannot insert object");
    let result = disk_cache
        .get(&b"skid row")
        .expect("cannot get object from disk")
        .expect("object not found in disk cache");
    assert_eq!(&result[..], b"slave to the grind");
}
