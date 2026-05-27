//! File set — a group of files belonging to the same source root.
//!
//! prefix-based file set classification, matching ra's implementation.

use fst::{IntoStreamer, Streamer};
use indexmap::IndexMap;
use rustc_hash::{FxBuildHasher, FxHashMap};

use crate::{AnchoredPath, FileId, Vfs, VfsPath};

/// A set of files belonging to a single source root.
#[derive(Default, Clone, Eq, PartialEq)]
pub struct FileSet {
    files: FxHashMap<VfsPath, FileId>,
    paths: IndexMap<FileId, VfsPath, FxBuildHasher>,
}

impl FileSet {
    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Resolve a path relative to an anchor file.
    pub fn resolve_path(&self, path: AnchoredPath<'_>) -> Option<FileId> {
        let mut base = self.paths[&path.anchor].clone();
        base.pop();
        let path = base.join(path.path)?;
        self.files.get(&path).copied()
    }

    pub fn file_for_path(&self, path: &VfsPath) -> Option<&FileId> {
        self.files.get(path)
    }

    pub fn path_for_file(&self, file: &FileId) -> Option<&VfsPath> {
        self.paths.get(file)
    }

    pub fn insert(&mut self, file_id: FileId, path: VfsPath) {
        self.files.insert(path.clone(), file_id);
        self.paths.insert(file_id, path);
    }

    pub fn iter(&self) -> impl Iterator<Item = FileId> + '_ {
        self.paths.keys().copied()
    }
}

impl std::fmt::Debug for FileSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileSet").field("n_files", &self.files.len()).finish()
    }
}

/// Configuration for partitioning VFS files into disjoint `FileSet`s.
///
/// matching ra's implementation. Includes an implicit overflow bucket
/// (n_file_sets = roots.len() + 1) for unmatched files.
#[derive(Debug)]
pub struct FileSetConfig {
    n_file_sets: usize,
    map: fst::Map<Vec<u8>>,
}

impl Default for FileSetConfig {
    fn default() -> Self {
        FileSetConfig::builder().build()
    }
}

impl FileSetConfig {
    pub fn builder() -> FileSetConfigBuilder {
        FileSetConfigBuilder::default()
    }

    /// Partition files from a VFS into disjoint file sets.
    pub fn partition(&self, vfs: &Vfs) -> Vec<FileSet> {
        let mut scratch_space = Vec::new();
        let mut result = vec![FileSet::default(); self.len()];
        for (file_id, path) in vfs.iter() {
            let idx = self.classify(path, &mut scratch_space);
            result[idx].insert(file_id, path.clone());
        }
        result
    }

    fn len(&self) -> usize {
        self.n_file_sets
    }

    /// Get the lexicographically ordered entries of the underlying FST map.
    pub fn roots(&self) -> Vec<(Vec<u8>, u64)> {
        self.map.stream().into_byte_vec()
    }

    /// Classify a path into a file set index using FST prefix matching.
    ///
    /// to avoid `/foo/bar_baz.sail` matching root `/foo/bar`. Uses FST
    /// `PrefixOf` automaton to find the longest matching prefix.
    fn classify(&self, path: &VfsPath, scratch_space: &mut Vec<u8>) -> usize {
        // Use parent directory — we don't want file names to affect classification
        let path = path.parent().unwrap_or_else(|| path.clone());

        scratch_space.clear();
        path.encode(scratch_space);
        let automaton = PrefixOf::new(scratch_space.as_slice());
        let mut longest_prefix = self.len() - 1; // default: overflow bucket
        let mut stream = self.map.search(automaton).into_stream();
        while let Some((_, v)) = stream.next() {
            longest_prefix = v as usize;
        }
        longest_prefix
    }
}

/// Builder for `FileSetConfig`.
#[derive(Default)]
pub struct FileSetConfigBuilder {
    roots: Vec<Vec<VfsPath>>,
}

impl FileSetConfigBuilder {
    pub fn len(&self) -> usize {
        self.roots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }

    pub fn add_file_set(&mut self, roots: Vec<VfsPath>) {
        self.roots.push(roots);
    }

    /// Build the config.
    ///
    /// Adds +1 overflow bucket (n_file_sets = roots.len() + 1) so unmatched
    /// files go to the last set.
    pub fn build(self) -> FileSetConfig {
        let n_file_sets = self.roots.len() + 1;
        let map = {
            let mut entries = Vec::new();
            for (i, paths) in self.roots.into_iter().enumerate() {
                for p in paths {
                    let mut buf = Vec::new();
                    p.encode(&mut buf);
                    entries.push((buf, i as u64));
                }
            }
            entries.sort();
            entries.dedup_by(|(a, _), (b, _)| a == b);
            fst::Map::from_iter(entries).unwrap()
        };
        FileSetConfig { n_file_sets, map }
    }
}

/// of the query data. Used to find the longest matching root prefix.
struct PrefixOf<'a> {
    prefix_of: &'a [u8],
}

impl<'a> PrefixOf<'a> {
    fn new(prefix_of: &'a [u8]) -> Self {
        Self { prefix_of }
    }
}

impl fst::Automaton for PrefixOf<'_> {
    type State = usize;
    fn start(&self) -> usize {
        0
    }
    fn is_match(&self, &state: &usize) -> bool {
        state != !0
    }
    fn can_match(&self, &state: &usize) -> bool {
        state != !0
    }
    fn accept(&self, &state: &usize, byte: u8) -> usize {
        if self.prefix_of.get(state) == Some(&byte) { state + 1 } else { !0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_path(s: &str) -> VfsPath {
        VfsPath::new(paths::AbsPathBuf::assert(paths::Utf8PathBuf::from(s)))
    }

    #[test]
    fn file_set_insert_and_lookup() {
        let mut fs = FileSet::default();
        let path = make_path("/project/test.sail");
        let id = FileId::from_raw(0);
        fs.insert(id, path.clone());

        assert_eq!(fs.len(), 1);
        assert_eq!(fs.file_for_path(&path), Some(&id));
        assert_eq!(fs.path_for_file(&id), Some(&path));
    }

    #[test]
    fn file_set_config_partitions() {
        let mut vfs = Vfs::default();
        let p1 = make_path("/local/a.sail");
        let p2 = make_path("/local/b.sail");
        let p3 = make_path("/lib/prelude.sail");
        vfs.set_file_contents(p1, Some(b"a".to_vec()));
        vfs.set_file_contents(p2, Some(b"b".to_vec()));
        vfs.set_file_contents(p3, Some(b"c".to_vec()));
        vfs.take_changes();

        let mut builder = FileSetConfig::builder();
        builder.add_file_set(vec![make_path("/local")]);
        builder.add_file_set(vec![make_path("/lib")]);
        let config = builder.build();
        let sets = config.partition(&vfs);

        assert_eq!(sets.len(), 3);
        assert_eq!(sets[0].len(), 2); // /local/a.sail, /local/b.sail
        assert_eq!(sets[1].len(), 1); // /lib/prelude.sail
        assert_eq!(sets[2].len(), 0); // overflow: empty
    }

    #[test]
    fn unmatched_files_go_to_overflow() {
        let mut vfs = Vfs::default();
        let p1 = make_path("/local/a.sail");
        let p2 = make_path("/other/b.sail");
        vfs.set_file_contents(p1, Some(b"a".to_vec()));
        vfs.set_file_contents(p2, Some(b"b".to_vec()));
        vfs.take_changes();

        let mut builder = FileSetConfig::builder();
        builder.add_file_set(vec![make_path("/local")]);
        let config = builder.build();
        let sets = config.partition(&vfs);

        assert_eq!(sets.len(), 2); // 1 defined + 1 overflow
        assert_eq!(sets[0].len(), 1); // /local/a.sail
        assert_eq!(sets[1].len(), 1); // /other/b.sail in overflow
    }

    #[test]
    fn resolve_path_relative_to_anchor() {
        let mut fs = FileSet::default();
        let anchor_path = make_path("/project/src/main.sail");
        let sibling_path = make_path("/project/src/utils.sail");
        let anchor_id = FileId::from_raw(0);
        let sibling_id = FileId::from_raw(1);
        fs.insert(anchor_id, anchor_path);
        fs.insert(sibling_id, sibling_path);

        let resolved = fs.resolve_path(AnchoredPath { anchor: anchor_id, path: "utils.sail" });
        assert_eq!(resolved, Some(sibling_id));
    }

    #[test]
    fn no_false_prefix_match() {
        // /foo/bar_baz.sail should NOT match root /foo/bar
        let mut vfs = Vfs::default();
        let p1 = make_path("/foo/bar/real.sail");
        let p2 = make_path("/foo/bar_baz.sail");
        vfs.set_file_contents(p1, Some(b"a".to_vec()));
        vfs.set_file_contents(p2, Some(b"b".to_vec()));
        vfs.take_changes();

        let mut builder = FileSetConfig::builder();
        builder.add_file_set(vec![make_path("/foo/bar")]);
        let config = builder.build();
        let sets = config.partition(&vfs);

        assert_eq!(sets[0].len(), 1); // only /foo/bar/real.sail
        assert_eq!(sets[1].len(), 1); // /foo/bar_baz.sail in overflow
    }
}
