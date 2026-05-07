//! Pure-gix shadow-ref operations.
//!
//! Net-new domain code for lineage. Captures the working tree (including
//! untracked files, honoring .gitignore) into a fresh tree object, writes a
//! commit, and updates a shadow ref under `refs/iii/lineage/checkpoints/v0/...`.
//! No subprocess fork-execs. Non-blocking by virtue of gix being native Rust.

use crate::{
    AttachTrailersInput, AttachTrailersOutput, ListShadowRefsInput, ResolveBlobInput,
    ResolveBlobOutput, RewindInput, SHADOW_REF_PREFIX, ShadowRefEntry, ShadowSnapshotInput,
    ShadowSnapshotOutput,
};
use anyhow::{Context, Result, anyhow};
use base64::Engine as _;
use gix::bstr::{BString, ByteSlice};
use gix::objs::tree::{Entry as ObjEntry, EntryKind};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use uuid::Uuid;

const ENTRY_ID_LEN: usize = 12;
const COMMITTER_NAME: &str = "lineage";
const COMMITTER_EMAIL: &str = "lineage@iii.dev";

fn entry_id() -> String {
    Uuid::new_v4().simple().to_string()[..ENTRY_ID_LEN].to_string()
}

fn shadow_ref_for(session_id: &str, entry_id: &str) -> String {
    format!("{SHADOW_REF_PREFIX}/{session_id}/{entry_id}")
}

fn open_repo(repo_path: &Path) -> Result<gix::Repository> {
    gix::open(repo_path).with_context(|| format!("open repo at {}", repo_path.display()))
}

fn signature_now() -> Result<gix::actor::Signature> {
    let time = gix::date::Time::now_local_or_utc();
    Ok(gix::actor::Signature {
        name: BString::from(COMMITTER_NAME),
        email: BString::from(COMMITTER_EMAIL),
        time,
    })
}

/// Recursive worktree → tree builder.
///
/// Walks `dir` honoring .gitignore (via the `ignore` crate). Hashes every
/// regular file as a blob and inserts it into the tree. Recurses into
/// subdirectories. Returns the OID of the tree object representing `dir`.
fn build_tree_from_dir(repo: &gix::Repository, dir: &Path) -> Result<Option<gix::ObjectId>> {
    // Group entries by their immediate child name. We use BTreeMap because gix
    // requires tree entries sorted by filename.
    let mut entries: BTreeMap<BString, ObjEntry> = BTreeMap::new();

    let walker = ignore::WalkBuilder::new(dir)
        .max_depth(Some(1))
        .hidden(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(false)
        .ignore(false)
        .parents(false)
        .filter_entry(|e| e.file_name() != ".git")
        .build();

    for result in walker {
        let dent = match result {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(error = %e, "ignore walker error, skipping entry");
                continue;
            }
        };
        if dent.path() == dir {
            continue;
        }
        let name = dent.file_name();
        let name_bytes = match name.to_str() {
            Some(s) => BString::from(s.as_bytes()),
            None => continue,
        };
        let path = dent.path();
        let file_type = match dent.file_type() {
            Some(t) => t,
            None => continue,
        };

        if file_type.is_dir() {
            if let Some(sub_oid) = build_tree_from_dir(repo, path)? {
                entries.insert(
                    name_bytes,
                    ObjEntry {
                        mode: EntryKind::Tree.into(),
                        filename: name_bytes_clone(path),
                        oid: sub_oid,
                    },
                );
            }
            // Empty subtrees are skipped — git doesn't track empty dirs anyway.
        } else if file_type.is_symlink() {
            let target =
                std::fs::read_link(path).with_context(|| format!("readlink {}", path.display()))?;
            let bytes = target.as_os_str().to_string_lossy();
            let oid = repo
                .write_blob(bytes.as_bytes())
                .context("write symlink target as blob")?
                .detach();
            entries.insert(
                name_bytes.clone(),
                ObjEntry {
                    mode: EntryKind::Link.into(),
                    filename: name_bytes,
                    oid,
                },
            );
        } else if file_type.is_file() {
            let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
            let oid = repo
                .write_blob(&bytes)
                .with_context(|| format!("write blob for {}", path.display()))?
                .detach();
            let mode = if is_executable(path) {
                EntryKind::BlobExecutable
            } else {
                EntryKind::Blob
            };
            entries.insert(
                name_bytes.clone(),
                ObjEntry {
                    mode: mode.into(),
                    filename: name_bytes,
                    oid,
                },
            );
        }
    }

    if entries.is_empty() {
        return Ok(None);
    }

    let tree = gix::objs::Tree {
        entries: entries.into_values().collect(),
    };
    let tree_oid = repo
        .write_object(&tree)
        .context("write tree object")?
        .detach();
    Ok(Some(tree_oid))
}

fn name_bytes_clone(path: &Path) -> BString {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|s| BString::from(s.as_bytes()))
        .unwrap_or_default()
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    false
}

fn parent_commit_oid(repo: &gix::Repository, parent_ref: &str) -> Option<gix::ObjectId> {
    repo.find_reference(parent_ref)
        .ok()
        .and_then(|mut r| r.peel_to_id_in_place().ok())
        .map(|id| id.detach())
}

/// Snapshot the working directory onto a fresh shadow ref.
///
/// Captures untracked files (the bug that disqualified `git stash create`).
/// Honors `.gitignore`. Builds a complete tree, writes a commit, updates the
/// shadow ref under `refs/iii/lineage/checkpoints/v0/<session>/<entry>`.
pub fn shadow_snapshot(input: ShadowSnapshotInput) -> Result<ShadowSnapshotOutput> {
    let repo = open_repo(&PathBuf::from(&input.repo_path))?;
    let workdir = repo
        .work_dir()
        .ok_or_else(|| anyhow!("repo is bare; cannot snapshot without a working tree"))?
        .to_path_buf();

    let eid = entry_id();
    let new_ref = shadow_ref_for(&input.session_id, &eid);

    let parent_oid = input
        .parent_entry_id
        .as_ref()
        .and_then(|p| parent_commit_oid(&repo, &shadow_ref_for(&input.session_id, p)));

    let tree_oid = match build_tree_from_dir(&repo, &workdir)? {
        Some(oid) => oid,
        None => repo
            .write_object(gix::objs::Tree::empty())
            .context("write empty tree")?
            .detach(),
    };

    let sig = signature_now()?;
    let commit = gix::objs::Commit {
        tree: tree_oid,
        parents: parent_oid.into_iter().collect(),
        author: sig.clone(),
        committer: sig,
        encoding: None,
        message: BString::from(format!(
            "lineage checkpoint\n\nLineage-Session: {}\nLineage-Entry: {}\n",
            input.session_id, eid
        )),
        extra_headers: vec![],
    };
    let commit_oid = repo.write_object(&commit).context("write commit")?.detach();

    repo.reference(
        new_ref.as_str(),
        commit_oid,
        gix::refs::transaction::PreviousValue::MustNotExist,
        format!("lineage checkpoint for session {}", input.session_id),
    )
    .with_context(|| format!("update-ref {new_ref}"))?;

    Ok(ShadowSnapshotOutput {
        shadow_ref: new_ref,
        commit_oid: commit_oid.to_string(),
        tree_oid: tree_oid.to_string(),
        entry_id: eid,
    })
}

/// Restore the working tree to a previous shadow checkpoint.
///
/// Mirrors `git read-tree --reset -u <ref>`: replaces the working tree with
/// the contents of the shadow ref's tree. Refuses if there are uncommitted
/// changes unless `force=true`.
pub fn rewind_to(input: RewindInput) -> Result<()> {
    let repo = open_repo(&PathBuf::from(&input.repo_path))?;
    let workdir = repo
        .work_dir()
        .ok_or_else(|| anyhow!("repo is bare; cannot rewind without a working tree"))?
        .to_path_buf();

    if !input.force && working_tree_has_changes(&repo)? {
        return Err(anyhow!(
            "working tree has uncommitted changes; pass force=true to rewind anyway"
        ));
    }

    let target_id = repo
        .find_reference(input.shadow_ref.as_str())
        .with_context(|| format!("find shadow ref {}", input.shadow_ref))?
        .peel_to_id_in_place()
        .context("peel shadow ref")?
        .detach();
    let target_obj = repo.find_object(target_id).context("find target object")?;
    let target_tree = match target_obj.kind {
        gix::object::Kind::Commit => target_obj.into_commit().tree_id()?.detach(),
        gix::object::Kind::Tree => target_id,
        _ => return Err(anyhow!("shadow ref does not point at a commit or tree")),
    };

    materialise_tree(&repo, target_tree, &workdir)?;
    Ok(())
}

/// Replace the contents of `workdir` with the contents of the tree at `tree_oid`.
///
/// Lists the current set of files reachable through .gitignore-honoring walk,
/// then writes every blob in the new tree to disk and removes any leftovers.
fn materialise_tree(repo: &gix::Repository, tree_oid: gix::ObjectId, workdir: &Path) -> Result<()> {
    use std::collections::HashSet;

    // Files that should exist after this operation.
    let mut want: HashSet<PathBuf> = HashSet::new();
    write_tree_to_disk(repo, tree_oid, workdir, &mut want)?;

    // Remove files that are not in `want` but exist in the worktree.
    let walker = ignore::WalkBuilder::new(workdir)
        .hidden(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(false)
        .ignore(false)
        .parents(false)
        .filter_entry(|e| e.file_name() != ".git")
        .build();
    for result in walker {
        let dent = match result {
            Ok(d) => d,
            Err(_) => continue,
        };
        if !dent.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        if !want.contains(dent.path()) {
            std::fs::remove_file(dent.path()).ok();
        }
    }
    Ok(())
}

fn write_tree_to_disk(
    repo: &gix::Repository,
    tree_oid: gix::ObjectId,
    target: &Path,
    out: &mut std::collections::HashSet<PathBuf>,
) -> Result<()> {
    std::fs::create_dir_all(target).with_context(|| format!("mkdir {}", target.display()))?;
    let tree_obj = repo.find_object(tree_oid).context("find tree")?;
    let tree = tree_obj.into_tree();
    for entry in tree.decode()?.entries.iter() {
        let name = entry.filename.to_str_lossy();
        let path = target.join(name.as_ref());
        let kind: EntryKind = entry.mode.kind();
        match kind {
            EntryKind::Tree => {
                let sub_oid = entry.oid.to_owned();
                write_tree_to_disk(repo, sub_oid, &path, out)?;
            }
            EntryKind::Blob | EntryKind::BlobExecutable => {
                let blob = repo.find_object(entry.oid.to_owned())?.into_blob();
                std::fs::write(&path, &blob.data)
                    .with_context(|| format!("write {}", path.display()))?;
                #[cfg(unix)]
                if matches!(kind, EntryKind::BlobExecutable) {
                    use std::os::unix::fs::PermissionsExt;
                    let mut perms = std::fs::metadata(&path)?.permissions();
                    perms.set_mode(0o755);
                    std::fs::set_permissions(&path, perms)?;
                }
                out.insert(path);
            }
            EntryKind::Link => {
                let blob = repo.find_object(entry.oid.to_owned())?.into_blob();
                let target_path =
                    std::str::from_utf8(&blob.data).context("symlink target is not valid utf-8")?;
                if path.exists() || path.symlink_metadata().is_ok() {
                    std::fs::remove_file(&path).ok();
                }
                #[cfg(unix)]
                std::os::unix::fs::symlink(target_path, &path)
                    .with_context(|| format!("symlink {}", path.display()))?;
                out.insert(path);
            }
            EntryKind::Commit => {
                // Submodule reference. Skip — preserving submodule state is a
                // separate feature lineage doesn't claim to support in v0.
            }
        }
    }
    Ok(())
}

fn working_tree_has_changes(repo: &gix::Repository) -> Result<bool> {
    let head_tree_id = match repo.head_tree_id() {
        Ok(id) => id.detach(),
        Err(_) => return Ok(true), // no HEAD → any working-tree content counts as dirty
    };
    let workdir = repo
        .work_dir()
        .context("repo has no work_dir")?
        .to_path_buf();
    let live_tree = match build_tree_from_dir(repo, &workdir)? {
        Some(oid) => oid,
        None => repo
            .write_object(gix::objs::Tree::empty())
            .context("write empty tree")?
            .detach(),
    };
    Ok(live_tree != head_tree_id)
}

/// Append `Lineage-Session` + `Lineage-Path` trailers to a commit's message.
///
/// Rewrites `commit_oid` with the new message. If the commit was HEAD,
/// updates HEAD to point at the rewrite. Refuses if `entry_path` is empty.
pub fn attach_trailers(input: AttachTrailersInput) -> Result<AttachTrailersOutput> {
    if input.entry_path.is_empty() {
        return Err(anyhow!("entry_path must contain at least one entry id"));
    }
    let repo = open_repo(&PathBuf::from(&input.repo_path))?;
    let oid = gix::ObjectId::from_hex(input.commit_oid.as_bytes())
        .with_context(|| format!("parse commit oid {}", input.commit_oid))?;
    let commit_obj = repo.find_object(oid)?.into_commit();
    let cref = commit_obj.decode()?;

    let path_str = input.entry_path.join(",");
    let trailers = format!(
        "\nLineage-Session: {}\nLineage-Path: {}\n",
        input.session_id, path_str
    );
    let mut new_message: Vec<u8> = cref.message.to_vec();
    if !new_message.ends_with(b"\n") {
        new_message.push(b'\n');
    }
    new_message.extend_from_slice(trailers.as_bytes());

    let owned = gix::objs::Commit {
        tree: cref.tree(),
        parents: cref.parents().collect(),
        author: cref.author.to_owned(),
        committer: cref.committer.to_owned(),
        encoding: cref.encoding.map(|e| e.into()),
        message: BString::from(new_message),
        extra_headers: cref
            .extra_headers
            .iter()
            .map(|(k, v)| ((*k).into(), v.as_ref().into()))
            .collect(),
    };

    let new_oid = repo.write_object(&owned).context("write commit")?.detach();

    if let Ok(head_id) = repo.head_id() {
        if head_id.detach() == oid {
            repo.edit_reference(gix::refs::transaction::RefEdit {
                change: gix::refs::transaction::Change::Update {
                    log: gix::refs::transaction::LogChange {
                        mode: gix::refs::transaction::RefLog::AndReference,
                        force_create_reflog: false,
                        message: BString::from("lineage attach-trailers"),
                    },
                    expected: gix::refs::transaction::PreviousValue::Any,
                    new: gix::refs::Target::Object(new_oid),
                },
                name: "HEAD".try_into().unwrap(),
                deref: true,
            })?;
        }
    }

    Ok(AttachTrailersOutput {
        new_commit_oid: new_oid.to_string(),
    })
}

/// List shadow refs under `refs/iii/lineage/checkpoints/v0[/<session_id>]`.
pub fn list_shadow_refs(input: ListShadowRefsInput) -> Result<Vec<ShadowRefEntry>> {
    let repo = open_repo(&PathBuf::from(&input.repo_path))?;
    let prefix = match &input.session_id {
        Some(sid) => format!("{SHADOW_REF_PREFIX}/{sid}/"),
        None => format!("{SHADOW_REF_PREFIX}/"),
    };
    let mut entries = Vec::new();
    for r in repo.references()?.prefixed(prefix.as_str())? {
        let mut r = match r {
            Ok(r) => r,
            Err(_) => continue,
        };
        let oid = match r.peel_to_id_in_place() {
            Ok(id) => id.detach(),
            Err(_) => continue,
        };
        let refname = r.name().as_bstr().to_string();
        let needle = format!("{SHADOW_REF_PREFIX}/");
        let suffix = match refname.strip_prefix(&needle) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let mut sp = suffix.splitn(2, '/');
        let (Some(sid), Some(eid)) = (sp.next(), sp.next()) else {
            continue;
        };
        entries.push(ShadowRefEntry {
            shadow_ref: refname,
            session_id: sid.to_string(),
            entry_id: eid.to_string(),
            commit_oid: oid.to_string(),
        });
    }
    Ok(entries)
}

/// Read a single blob from the tree at `shadow_ref`.
pub fn resolve_blob(input: ResolveBlobInput) -> Result<ResolveBlobOutput> {
    let repo = open_repo(&PathBuf::from(&input.repo_path))?;
    let target_id = repo
        .find_reference(input.shadow_ref.as_str())
        .with_context(|| format!("find shadow ref {}", input.shadow_ref))?
        .peel_to_id_in_place()
        .context("peel shadow ref")?
        .detach();
    let mut tree_id = match repo.find_object(target_id)?.kind {
        gix::object::Kind::Commit => repo
            .find_object(target_id)?
            .into_commit()
            .tree_id()?
            .detach(),
        gix::object::Kind::Tree => target_id,
        _ => return Err(anyhow!("shadow ref does not point at a commit or tree")),
    };

    // Walk the path components.
    let parts: Vec<&str> = input.path.split('/').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return Err(anyhow!("path must not be empty"));
    }
    let mut blob_oid: Option<gix::ObjectId> = None;
    for (i, part) in parts.iter().enumerate() {
        let tree_obj = repo.find_object(tree_id)?.into_tree();
        let decoded = tree_obj.decode()?;
        let needle = part.as_bytes();
        let entry = decoded
            .entries
            .iter()
            .find(|e| {
                let filename: &[u8] = e.filename.as_ref();
                filename == needle
            })
            .ok_or_else(|| {
                anyhow!(
                    "path component {part} not found in tree {tree_id} (rooted at shadow ref {}).\nHint: list captured paths with `git ls-tree -r {tree_id}` or pick a known path from `git for-each-ref refs/iii/lineage/checkpoints/v0/`.",
                    input.shadow_ref
                )
            })?;
        let oid = entry.oid.to_owned();
        let kind: EntryKind = entry.mode.kind();
        if i == parts.len() - 1 {
            match kind {
                EntryKind::Blob | EntryKind::BlobExecutable | EntryKind::Link => {
                    blob_oid = Some(oid);
                }
                _ => return Err(anyhow!("path {} is not a blob", input.path)),
            }
        } else {
            if !matches!(kind, EntryKind::Tree) {
                return Err(anyhow!("path component {part} is not a tree"));
            }
            tree_id = oid;
        }
    }

    let blob_oid = blob_oid.ok_or_else(|| anyhow!("blob not found"))?;
    let blob = repo.find_object(blob_oid)?.into_blob();
    let size = blob.data.len() as u64;
    let content_base64 = base64::engine::general_purpose::STANDARD.encode(&blob.data);
    Ok(ResolveBlobOutput {
        blob_oid: blob_oid.to_string(),
        size,
        content_base64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    fn init_test_repo() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().to_path_buf();
        run(&repo, &["init", "-q", "-b", "main"]);
        run(&repo, &["config", "user.email", "test@example.com"]);
        run(&repo, &["config", "user.name", "test"]);
        run(&repo, &["config", "commit.gpgsign", "false"]);
        fs::write(repo.join("hello.txt"), "first\n").unwrap();
        run(&repo, &["add", "."]);
        run(&repo, &["commit", "-q", "-m", "init"]);
        (tmp, repo)
    }

    fn run(repo: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    #[test]
    fn shadow_snapshot_creates_ref_and_commit_chain() {
        let (_tmp, repo) = init_test_repo();
        fs::write(repo.join("hello.txt"), "second\n").unwrap();
        let s1 = shadow_snapshot(ShadowSnapshotInput {
            session_id: "s".into(),
            parent_entry_id: None,
            repo_path: repo.display().to_string(),
        })
        .unwrap();
        assert!(s1.shadow_ref.starts_with(SHADOW_REF_PREFIX));
        assert_eq!(s1.commit_oid.len(), 40);
        assert_eq!(s1.entry_id.len(), ENTRY_ID_LEN);

        fs::write(repo.join("hello.txt"), "third\n").unwrap();
        let s2 = shadow_snapshot(ShadowSnapshotInput {
            session_id: "s".into(),
            parent_entry_id: Some(s1.entry_id.clone()),
            repo_path: repo.display().to_string(),
        })
        .unwrap();
        let parents = run(&repo, &["log", "-1", "--format=%P", &s2.commit_oid]);
        assert_eq!(parents, s1.commit_oid);
    }

    #[test]
    fn shadow_snapshot_captures_untracked_files() {
        // CRITICAL TEST: the bug that disqualified `git stash create`.
        let (_tmp, repo) = init_test_repo();
        fs::write(repo.join("brand_new.txt"), "untracked content\n").unwrap();

        let snap = shadow_snapshot(ShadowSnapshotInput {
            session_id: "untracked".into(),
            parent_entry_id: None,
            repo_path: repo.display().to_string(),
        })
        .unwrap();

        // The snapshot tree must contain brand_new.txt.
        let blob = resolve_blob(ResolveBlobInput {
            repo_path: repo.display().to_string(),
            shadow_ref: snap.shadow_ref,
            path: "brand_new.txt".into(),
        })
        .unwrap();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(blob.content_base64)
            .unwrap();
        assert_eq!(String::from_utf8(decoded).unwrap(), "untracked content\n");
    }

    #[test]
    fn shadow_snapshot_honours_gitignore() {
        let (_tmp, repo) = init_test_repo();
        fs::write(repo.join(".gitignore"), "secret.env\n").unwrap();
        fs::write(repo.join("secret.env"), "API_KEY=should-not-be-captured\n").unwrap();
        fs::write(repo.join("public.txt"), "fine to capture\n").unwrap();

        let snap = shadow_snapshot(ShadowSnapshotInput {
            session_id: "ignored".into(),
            parent_entry_id: None,
            repo_path: repo.display().to_string(),
        })
        .unwrap();

        // public.txt is captured.
        let pub_blob = resolve_blob(ResolveBlobInput {
            repo_path: repo.display().to_string(),
            shadow_ref: snap.shadow_ref.clone(),
            path: "public.txt".into(),
        });
        assert!(pub_blob.is_ok());

        // secret.env is NOT captured.
        let sec_blob = resolve_blob(ResolveBlobInput {
            repo_path: repo.display().to_string(),
            shadow_ref: snap.shadow_ref,
            path: "secret.env".into(),
        });
        assert!(sec_blob.is_err());
    }

    #[test]
    fn rewind_restores_untracked_files_from_snapshot() {
        let (_tmp, repo) = init_test_repo();
        fs::write(repo.join("captured.txt"), "captured content\n").unwrap();
        let snap = shadow_snapshot(ShadowSnapshotInput {
            session_id: "untracked-rewind".into(),
            parent_entry_id: None,
            repo_path: repo.display().to_string(),
        })
        .unwrap();

        // Delete the captured file.
        fs::remove_file(repo.join("captured.txt")).unwrap();
        assert!(!repo.join("captured.txt").exists());

        rewind_to(RewindInput {
            repo_path: repo.display().to_string(),
            shadow_ref: snap.shadow_ref,
            force: true,
        })
        .unwrap();

        let restored = fs::read_to_string(repo.join("captured.txt")).unwrap();
        assert_eq!(restored, "captured content\n");
    }

    #[test]
    fn rewind_refuses_dirty_without_force() {
        let (_tmp, repo) = init_test_repo();
        fs::write(repo.join("hello.txt"), "second\n").unwrap();
        let snap = shadow_snapshot(ShadowSnapshotInput {
            session_id: "s".into(),
            parent_entry_id: None,
            repo_path: repo.display().to_string(),
        })
        .unwrap();
        fs::write(repo.join("hello.txt"), "dirty\n").unwrap();
        let err = rewind_to(RewindInput {
            repo_path: repo.display().to_string(),
            shadow_ref: snap.shadow_ref,
            force: false,
        })
        .unwrap_err();
        assert!(err.to_string().contains("uncommitted"));
    }

    #[test]
    fn list_shadow_refs_returns_session_entries() {
        let (_tmp, repo) = init_test_repo();
        fs::write(repo.join("hello.txt"), "second\n").unwrap();
        shadow_snapshot(ShadowSnapshotInput {
            session_id: "alpha".into(),
            parent_entry_id: None,
            repo_path: repo.display().to_string(),
        })
        .unwrap();
        fs::write(repo.join("hello.txt"), "third\n").unwrap();
        shadow_snapshot(ShadowSnapshotInput {
            session_id: "beta".into(),
            parent_entry_id: None,
            repo_path: repo.display().to_string(),
        })
        .unwrap();
        let all = list_shadow_refs(ListShadowRefsInput {
            repo_path: repo.display().to_string(),
            session_id: None,
        })
        .unwrap();
        assert_eq!(all.len(), 2);
        let alpha = list_shadow_refs(ListShadowRefsInput {
            repo_path: repo.display().to_string(),
            session_id: Some("alpha".into()),
        })
        .unwrap();
        assert_eq!(alpha.len(), 1);
        assert_eq!(alpha[0].session_id, "alpha");
    }

    #[test]
    fn attach_trailers_rewrites_message() {
        let (_tmp, repo) = init_test_repo();
        let head = run(&repo, &["rev-parse", "HEAD"]);
        let out = attach_trailers(AttachTrailersInput {
            repo_path: repo.display().to_string(),
            commit_oid: head.clone(),
            session_id: "sess-1".into(),
            entry_path: vec!["a".into(), "b".into()],
        })
        .unwrap();
        assert_ne!(out.new_commit_oid, head);
        let body = run(&repo, &["log", "-1", "--format=%B", &out.new_commit_oid]);
        assert!(body.contains("Lineage-Session: sess-1"));
        assert!(body.contains("Lineage-Path: a,b"));
    }

    #[test]
    fn attach_trailers_rejects_empty_entry_path() {
        let (_tmp, repo) = init_test_repo();
        let head = run(&repo, &["rev-parse", "HEAD"]);
        let err = attach_trailers(AttachTrailersInput {
            repo_path: repo.display().to_string(),
            commit_oid: head,
            session_id: "sess".into(),
            entry_path: vec![],
        })
        .unwrap_err();
        assert!(err.to_string().contains("entry_path"));
    }

    #[test]
    fn resolve_blob_reads_committed_content() {
        let (_tmp, repo) = init_test_repo();
        fs::write(repo.join("hello.txt"), "captured\n").unwrap();
        let snap = shadow_snapshot(ShadowSnapshotInput {
            session_id: "rb".into(),
            parent_entry_id: None,
            repo_path: repo.display().to_string(),
        })
        .unwrap();
        let blob = resolve_blob(ResolveBlobInput {
            repo_path: repo.display().to_string(),
            shadow_ref: snap.shadow_ref,
            path: "hello.txt".into(),
        })
        .unwrap();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(blob.content_base64)
            .unwrap();
        assert_eq!(String::from_utf8(decoded).unwrap(), "captured\n");
        assert_eq!(blob.size, 9);
    }

    #[test]
    fn resolve_blob_errors_on_missing_path() {
        let (_tmp, repo) = init_test_repo();
        let snap = shadow_snapshot(ShadowSnapshotInput {
            session_id: "miss".into(),
            parent_entry_id: None,
            repo_path: repo.display().to_string(),
        })
        .unwrap();
        let err = resolve_blob(ResolveBlobInput {
            repo_path: repo.display().to_string(),
            shadow_ref: snap.shadow_ref,
            path: "no_such.txt".into(),
        })
        .unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn shadow_snapshot_errors_on_non_git_path() {
        let tmp = tempfile::tempdir().unwrap();
        // tmp is NOT a git repo.
        let err = shadow_snapshot(ShadowSnapshotInput {
            session_id: "s".into(),
            parent_entry_id: None,
            repo_path: tmp.path().display().to_string(),
        })
        .unwrap_err();
        // Don't pin exact wording; gix returns "could not find a git repository" or similar.
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn shadow_snapshot_works_on_fresh_repo_with_no_head() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().to_path_buf();
        run(&repo, &["init", "-q", "-b", "main"]);
        fs::write(repo.join("only.txt"), "one\n").unwrap();
        // No commit yet.
        let snap = shadow_snapshot(ShadowSnapshotInput {
            session_id: "fresh".into(),
            parent_entry_id: None,
            repo_path: repo.display().to_string(),
        })
        .unwrap();
        let blob = resolve_blob(ResolveBlobInput {
            repo_path: repo.display().to_string(),
            shadow_ref: snap.shadow_ref,
            path: "only.txt".into(),
        })
        .unwrap();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(blob.content_base64)
            .unwrap();
        assert_eq!(String::from_utf8(decoded).unwrap(), "one\n");
    }

    #[test]
    fn rewind_errors_on_missing_ref() {
        let (_tmp, repo) = init_test_repo();
        let err = rewind_to(RewindInput {
            repo_path: repo.display().to_string(),
            shadow_ref: format!("{SHADOW_REF_PREFIX}/never/ existed"),
            force: true,
        })
        .unwrap_err();
        assert!(!err.to_string().is_empty());
    }
}
