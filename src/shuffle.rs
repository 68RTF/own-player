use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Default)]
pub(crate) struct ShuffleOrder {
    played: HashSet<PathBuf>,
    history: Vec<PathBuf>,
    cursor: Option<usize>,
}

impl ShuffleOrder {
    pub(crate) fn record(&mut self, path: &Path) {
        self.played.insert(path.to_path_buf());
        let length = self.cursor.map_or(0, |cursor| cursor + 1);
        if length > 0 && self.history[length - 1] == path {
            return;
        }
        self.history.truncate(length);
        self.history.push(path.to_path_buf());
        self.cursor = Some(length);
    }

    pub(crate) fn has_previous(&self) -> bool {
        self.cursor.is_some_and(|cursor| cursor > 0)
    }

    pub(crate) fn previous(&mut self) -> Option<&Path> {
        let previous = self.cursor?.checked_sub(1)?;
        self.cursor = Some(previous);
        Some(&self.history[previous])
    }

    pub(crate) fn can_advance<'a>(&self, tracks: impl Iterator<Item = &'a Path>) -> bool {
        self.cursor
            .is_some_and(|cursor| cursor + 1 < self.history.len())
            || tracks.into_iter().any(|path| !self.played.contains(path))
    }

    pub(crate) fn next<'a>(
        &mut self,
        tracks: impl Iterator<Item = &'a Path>,
        current: &Path,
        repeat: bool,
    ) -> Option<PathBuf> {
        let forward = self.cursor.map_or(0, |cursor| cursor + 1);
        if let Some(path) = self.history.get(forward) {
            self.cursor = Some(forward);
            return Some(path.clone());
        }

        let tracks: Vec<_> = tracks.collect();
        let mut candidates: Vec<_> = tracks
            .iter()
            .copied()
            .filter(|path| !self.played.contains(*path))
            .collect();
        if candidates.is_empty() && repeat {
            self.played.clear();
            candidates.extend(tracks.iter().copied().filter(|path| *path != current));
            if candidates.is_empty() {
                candidates.extend(tracks);
            }
        }
        if candidates.is_empty() {
            return None;
        }
        let pick = glib::random_int_range(0, candidates.len() as i32);
        Some(candidates[pick as usize].to_path_buf())
    }

    pub(crate) fn remove(&mut self, path: &Path) {
        self.played.remove(path);
        let keep_through_cursor = self.cursor.map_or(0, |cursor| {
            self.history[..=cursor]
                .iter()
                .filter(|track| *track != path)
                .count()
        });
        self.history.retain(|track| track != path);
        self.cursor = keep_through_cursor.checked_sub(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plays_each_track_once_and_replays_history_in_both_directions() {
        let tracks = [Path::new("a"), Path::new("b"), Path::new("c")];
        let mut order = ShuffleOrder::default();
        order.record(tracks[0]);
        let second = order.next(tracks.into_iter(), tracks[0], false).unwrap();
        order.record(&second);
        let third = order.next(tracks.into_iter(), &second, false).unwrap();
        order.record(&third);
        assert_ne!(second, third);
        assert_ne!(third, tracks[0]);
        assert!(order.next(tracks.into_iter(), &third, false).is_none());
        assert_eq!(order.previous(), Some(second.as_path()));
        assert_eq!(order.previous(), Some(tracks[0]));
        assert_eq!(
            order.next(tracks.into_iter(), tracks[0], false),
            Some(second)
        );
        assert_eq!(
            order.next(tracks.into_iter(), tracks[0], false),
            Some(third.clone())
        );
        assert_ne!(order.next(tracks.into_iter(), &third, true), Some(third));
    }

    #[test]
    fn removing_a_track_prunes_history_and_new_tracks_enter_the_current_cycle() {
        let mut order = ShuffleOrder::default();
        let a = Path::new("a");
        let b = Path::new("b");
        let c = Path::new("c");
        order.record(a);
        order.record(b);
        order.remove(a);
        assert!(!order.has_previous());
        assert_eq!(
            order.next([b, c].into_iter(), b, false),
            Some(c.to_path_buf())
        );
    }
}
