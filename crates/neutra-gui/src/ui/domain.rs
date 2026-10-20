use serde::{Deserialize, Serialize};

pub(crate) const MENU_H: f32 = 30.0;
pub(crate) const QUERY_H: f32 = 44.0;
pub(crate) const FILTER_H: f32 = 34.0;
pub(crate) const TOOLBAR_H: f32 = 38.0;
pub(crate) const BANNER_H: f32 = 44.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ResultView {
    #[default]
    Details,
    List,
    Grid,
    Treemap,
}

impl ResultView {
    pub(crate) const ALL: [Self; 4] = [Self::Details, Self::List, Self::Grid, Self::Treemap];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Details => "Details",
            Self::List => "List",
            Self::Grid => "Grid",
            Self::Treemap => "Treemap",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum KindFilter {
    #[default]
    All,
    Audio,
    Images,
    Video,
    Programs,
    Compressed,
    Documents,
    Folders,
    /// Kept so older settings still load; the button row no longer offers
    /// it, and startup migrates it to `All` (see app state).
    Files,
}

impl KindFilter {
    pub(crate) const ALL: [Self; 8] = [
        Self::All,
        Self::Audio,
        Self::Images,
        Self::Video,
        Self::Programs,
        Self::Compressed,
        Self::Documents,
        Self::Folders,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Audio => "Audio",
            Self::Images => "Images",
            Self::Video => "Video",
            Self::Programs => "Programs",
            Self::Compressed => "Archives",
            Self::Documents => "Documents",
            Self::Folders => "Folders",
            Self::Files => "Files",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum SortMode {
    /// Engine relevance (name-prefix > name > path, then mtime). With no
    /// query text every score is equal, so this degrades to newest-first.
    #[default]
    Relevance,
    Modified,
    Name,
    Size,
    Path,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum SearchMode {
    /// File/folder names only. The default: directory paths never match
    /// unless the user picks one of the path options below.
    #[default]
    Name,
    NameAndPath,
    Path,
}

impl SearchMode {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Name => "File names only",
            Self::NameAndPath => "Names + folder paths",
            Self::Path => "Folder paths only",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RuntimeState {
    Ready,
    FirstRun,
    IndexingInitial,
    IndexingBackground,
    Permission,
    Stale,
}
