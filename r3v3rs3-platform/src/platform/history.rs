//! The archive timelines of a database. An in-place recovery starts a new history: the
//! recovered server numbers its log files from the start again, so the archive of the old
//! history moves into `history-<unix ms of the switch>/` next to the current archive. A
//! recovery takes the timeline that held the database at the target time.

use super::Platform;
use super::backup::segments_in;
use super::wal::{BaseBackup, bases_in};
use crate::clock::unix_ms;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::WalSegment;
use std::path::{Path, PathBuf};

const HISTORY_PREFIX: &str = "history-";

/// The suffix of a history directory whose move is not finished.
const MOVING_SUFFIX: &str = ".moving";

/// The directories of the current archive, which a history directory holds too.
const ARCHIVE_DIRS: [&str; 2] = ["wal", "base"];

/// One archive timeline: its log files, its base backups and, for a history, the time of the
/// switch that ended it.
pub(super) struct Archive {
    pub(super) wal: PathBuf,
    pub(super) base: PathBuf,
    pub(super) end: Option<u64>,
}

impl Archive {
    fn in_dir(dir: &Path, end: Option<u64>) -> Self {
        Self {
            wal: dir.join(ARCHIVE_DIRS[0]),
            base: dir.join(ARCHIVE_DIRS[1]),
            end,
        }
    }

    pub(super) async fn bases(&self) -> anyhow::Result<Vec<BaseBackup>> {
        bases_in(&self.base).await
    }

    pub(super) async fn segments(&self) -> anyhow::Result<Vec<WalSegment>> {
        let mut segments = segments_in(&self.wal).await?;
        segments.retain(|s| s.native);
        Ok(segments)
    }
}

/// The index of the timeline that held the database at `target`, given the end times of the
/// histories in ascending order: the first history that ended after the target, or the current
/// archive (the index after the last history).
pub(super) fn timeline_for(ends: &[u64], target: u64) -> usize {
    ends.iter()
        .position(|&end| end > target)
        .unwrap_or(ends.len())
}

/// The end time in the name of a finished history directory.
fn history_end(name: &str) -> Option<u64> {
    name.strip_prefix(HISTORY_PREFIX)?.parse().ok()
}

/// The history directories to remove when the retention keeps `keep` base backups over all
/// timelines, newest first: a history goes when none of its bases is among the kept ones.
/// `histories` holds the base count of each history, oldest first. The first base of the
/// current timeline does not count: a recovery takes it at once, and with `keep = 1` it would
/// remove the history of the recovery in the same call.
fn expired_histories(current_bases: usize, histories: &[usize], keep: usize) -> Vec<usize> {
    let mut left = keep.max(1).saturating_sub(current_bases.saturating_sub(1));
    let mut expired = Vec::new();
    for (index, &count) in histories.iter().enumerate().rev() {
        if left == 0 {
            expired.push(index);
        }
        left = left.saturating_sub(count);
    }
    expired
}

impl Platform {
    /// Every archive timeline of a database: the histories, oldest first, then the current
    /// archive.
    pub(super) async fn archives(&self, db_id: ShortId) -> anyhow::Result<Vec<Archive>> {
        let root = self.backup_dir(db_id).await?;
        let mut ends = history_ends(&root).await?;
        ends.sort_unstable();
        let mut archives: Vec<Archive> = ends
            .into_iter()
            .map(|end| Archive::in_dir(&root.join(format!("{HISTORY_PREFIX}{end}")), Some(end)))
            .collect();
        archives.push(Archive::in_dir(&root, None));
        Ok(archives)
    }

    /// Moves the current archive into a new history directory. A crash in the middle leaves a
    /// `.moving` directory that `finish_history_moves` completes before the archive is used.
    pub(super) async fn start_history(&self, db_id: ShortId) -> anyhow::Result<()> {
        let root = self.backup_dir(db_id).await?;
        let mut archived = false;
        for dir in ARCHIVE_DIRS {
            archived |= tokio::fs::try_exists(root.join(dir)).await?;
        }
        if !archived {
            return Ok(());
        }
        let moving = root.join(format!("{HISTORY_PREFIX}{}{MOVING_SUFFIX}", unix_ms()));
        tokio::fs::create_dir_all(&moving).await?;
        finish_moves(&root).await
    }

    /// Completes a move into a history directory that a crash interrupted. Nothing writes the
    /// current archive between the start of a move and this call, so the files left in it
    /// still belong to the old history.
    pub(super) async fn finish_history_moves(&self, db_id: ShortId) -> anyhow::Result<()> {
        finish_moves(&self.backup_dir(db_id).await?).await
    }

    /// Removes the history directories that hold none of the newest `keep` base backups.
    pub(super) async fn prune_histories(&self, db_id: ShortId, keep: usize) -> anyhow::Result<()> {
        let archives = self.archives(db_id).await?;
        let Some((current, histories)) = archives.split_last() else {
            return Ok(());
        };
        let current_bases = current.bases().await?.len();
        let mut counts = Vec::new();
        for history in histories {
            counts.push(history.bases().await?.len());
        }
        let root = self.backup_dir(db_id).await?;
        for index in expired_histories(current_bases, &counts, keep) {
            if let Some(end) = histories[index].end {
                tokio::fs::remove_dir_all(root.join(format!("{HISTORY_PREFIX}{end}"))).await?;
            }
        }
        Ok(())
    }
}

/// The end times of the finished history directories under `root`.
async fn history_ends(root: &Path) -> anyhow::Result<Vec<u64>> {
    let mut ends = Vec::new();
    let mut reader = match tokio::fs::read_dir(root).await {
        Ok(reader) => reader,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(ends),
        Err(err) => return Err(err.into()),
    };
    while let Some(entry) = reader.next_entry().await? {
        if let Some(end) = entry.file_name().to_str().and_then(history_end) {
            ends.push(end);
        }
    }
    Ok(ends)
}

/// Moves the current archive into every unfinished history directory under `root` and gives
/// the directory its final name.
async fn finish_moves(root: &Path) -> anyhow::Result<()> {
    let mut reader = match tokio::fs::read_dir(root).await {
        Ok(reader) => reader,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err.into()),
    };
    while let Some(entry) = reader.next_entry().await? {
        let name = entry.file_name();
        let Some(done) = name.to_str().and_then(|n| n.strip_suffix(MOVING_SUFFIX)) else {
            continue;
        };
        let moving = entry.path();
        move_current_into(root, &moving).await?;
        tokio::fs::rename(&moving, root.join(done)).await?;
    }
    Ok(())
}

/// Renames the directories of the current archive into `history`, each one only once.
async fn move_current_into(root: &Path, history: &Path) -> anyhow::Result<()> {
    for dir in ARCHIVE_DIRS {
        let from = root.join(dir);
        let to = history.join(dir);
        if tokio::fs::try_exists(&from).await? && !tokio::fs::try_exists(&to).await? {
            tokio::fs::rename(&from, &to).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A history holds the database until the switch that ended it, so a target before the
    /// end of the first history goes to that history, and a target after the last switch to
    /// the current archive. A target at the switch itself lies on the new timeline.
    #[test]
    fn the_target_time_picks_the_timeline_that_held_the_database() {
        let ends = [1_000, 5_000];
        assert_eq!(timeline_for(&ends, 500), 0);
        assert_eq!(timeline_for(&ends, 999), 0);
        assert_eq!(timeline_for(&ends, 1_000), 1);
        assert_eq!(timeline_for(&ends, 4_999), 1);
        assert_eq!(timeline_for(&ends, 5_000), 2);
        assert_eq!(timeline_for(&ends, 9_000), 2);
        assert_eq!(timeline_for(&[], 9_000), 0);
    }

    /// The retention keeps `keep` base backups over all timelines, newest first, without the
    /// first base of the current timeline. With three kept and two current bases, the two
    /// newest histories keep their bases and the oldest one goes.
    #[test]
    fn a_history_goes_when_none_of_its_bases_is_kept() {
        assert_eq!(expired_histories(2, &[1, 2, 1], 3), vec![0]);
        assert_eq!(expired_histories(4, &[1, 1], 3), vec![1, 0]);
        assert!(expired_histories(0, &[1, 1], 5).is_empty());
    }

    /// With `keep = 1`, the base that a recovery takes right after the move leaves the history
    /// of that recovery in place; the next base of the new timeline removes it.
    #[test]
    fn the_first_base_after_a_recovery_keeps_its_history() {
        assert!(expired_histories(1, &[1], 1).is_empty());
        assert_eq!(expired_histories(2, &[1], 1), vec![0]);
    }

    /// Only finished history directories count: an unfinished move and other names do not.
    #[test]
    fn only_a_finished_history_directory_has_an_end() {
        assert_eq!(
            history_end("history-1795000123000"),
            Some(1_795_000_123_000)
        );
        assert_eq!(history_end("history-1795000123000.moving"), None);
        assert_eq!(history_end("wal"), None);
    }

    /// A move that a crash cut off after the log files finishes with the base backups, and the
    /// history gets its final name.
    #[tokio::test]
    async fn an_interrupted_move_finishes() -> anyhow::Result<()> {
        let root =
            std::env::temp_dir().join(format!("r3-history-{}", super::super::backup::backup_id()));
        let moving = root.join("history-7.moving");
        tokio::fs::create_dir_all(moving.join("wal")).await?;
        tokio::fs::create_dir_all(root.join("base")).await?;
        tokio::fs::write(root.join("base/b.json"), b"{}").await?;
        finish_moves(&root).await?;
        assert!(tokio::fs::try_exists(root.join("history-7/base/b.json")).await?);
        assert!(tokio::fs::try_exists(root.join("history-7/wal")).await?);
        assert!(!tokio::fs::try_exists(root.join("base")).await?);
        assert!(!tokio::fs::try_exists(&moving).await?);
        tokio::fs::remove_dir_all(&root).await?;
        Ok(())
    }
}
