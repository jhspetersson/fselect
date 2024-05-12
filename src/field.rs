//! Defines the various fields available in the query language

use std::fmt::Display;
use std::fmt::Error;
use std::fmt::Formatter;
use std::str::FromStr;

use serde::ser::{Serialize, Serializer};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Hash)]
pub enum Field {
    Name,
    Path,
    AbsPath,
    Extension,
    Directory,
    AbsDir,
    Size,
    FormattedSize,
    Uid,
    Gid,
    #[cfg(all(unix, feature = "users"))]
    User,
    #[cfg(all(unix, feature = "users"))]
    Group,
    Created,
    Accessed,
    Modified,
    IsDir,
    IsFile,
    IsSymlink,
    IsPipe,
    IsCharacterDevice,
    IsBlockDevice,
    IsSocket,
    Device,
    Inode,
    Blocks,
    Hardlinks,
    Mode,
    UserRead,
    UserWrite,
    UserExec,
    UserAll,
    GroupRead,
    GroupWrite,
    GroupExec,
    GroupAll,
    OtherRead,
    OtherWrite,
    OtherExec,
    OtherAll,
    Suid,
    Sgid,
    IsHidden,
    HasXattrs,
    Capabilities,
    IsShebang,
    IsEmpty,
    Width,
    Height,
    Duration,
    Bitrate,
    Freq,
    Title,
    Artist,
    Album,
    Year,
    Genre,
    ExifDateTime,
    ExifGpsAltitude,
    ExifGpsLatitude,
    ExifGpsLongitude,
    ExifMake,
    ExifModel,
    ExifSoftware,
    ExifVersion,
    Mime,
    LineCount,
    IsBinary,
    IsText,
    IsArchive,
    IsAudio,
    IsBook,
    IsDoc,
    IsFont,
    IsImage,
    IsSource,
    IsVideo,
    Sha1,
    Sha256,
    Sha512,
    Sha3,
}

impl FromStr for Field {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let field = s.to_ascii_lowercase();

        match field.as_str() {
            "name" => Ok(Field::Name),
            "ext" | "extension" => Ok(Field::Extension),
            "path" => Ok(Field::Path),
            "abspath" => Ok(Field::AbsPath),
            "dir" | "directory" | "dirname" => Ok(Field::Directory),
            "absdir" => Ok(Field::AbsDir),
            "size" => Ok(Field::Size),
            "fsize" | "hsize" => Ok(Field::FormattedSize),
            "uid" => Ok(Field::Uid),
            "gid" => Ok(Field::Gid),
            #[cfg(all(unix, feature = "users"))]
            "user" => Ok(Field::User),
            #[cfg(all(unix, feature = "users"))]
            "group" => Ok(Field::Group),
            "created" => Ok(Field::Created),
            "accessed" => Ok(Field::Accessed),
            "modified" => Ok(Field::Modified),
            "is_dir" => Ok(Field::IsDir),
            "is_file" => Ok(Field::IsFile),
            "is_symlink" => Ok(Field::IsSymlink),
            "is_pipe" | "is_fifo" => Ok(Field::IsPipe),
            "is_char" | "is_character" => Ok(Field::IsCharacterDevice),
            "is_block" => Ok(Field::IsBlockDevice),
            "is_socket" => Ok(Field::IsSocket),
            "device" => Ok(Field::Device),
            "inode" => Ok(Field::Inode),
            "blocks" => Ok(Field::Blocks),
            "hardlinks" => Ok(Field::Hardlinks),
            "mode" => Ok(Field::Mode),
            "user_read" => Ok(Field::UserRead),
            "user_write" => Ok(Field::UserWrite),
            "user_exec" => Ok(Field::UserExec),
            "user_all" | "user_rwx" => Ok(Field::UserAll),
            "group_read" => Ok(Field::GroupRead),
            "group_write" => Ok(Field::GroupWrite),
            "group_exec" => Ok(Field::GroupExec),
            "group_all" | "group_rwx" => Ok(Field::GroupAll),
            "other_read" => Ok(Field::OtherRead),
            "other_write" => Ok(Field::OtherWrite),
            "other_exec" => Ok(Field::OtherExec),
            "other_all" | "other_rwx" => Ok(Field::OtherAll),
            "suid" => Ok(Field::Suid),
            "sgid" => Ok(Field::Sgid),
            "is_hidden" => Ok(Field::IsHidden),
            "has_xattrs" => Ok(Field::HasXattrs),
            "capabilities" | "caps" => Ok(Field::Capabilities),
            "is_shebang" => Ok(Field::IsShebang),
            "is_empty" => Ok(Field::IsEmpty),
            "width" => Ok(Field::Width),
            "height" => Ok(Field::Height),
            "mime" => Ok(Field::Mime),
            "line_count" => Ok(Field::LineCount),
            "duration" => Ok(Field::Duration),
            "mp3_bitrate" | "bitrate" => Ok(Field::Bitrate),
            "mp3_freq" | "freq" => Ok(Field::Freq),
            "mp3_title" | "title" => Ok(Field::Title),
            "mp3_artist" | "artist" => Ok(Field::Artist),
            "mp3_album" | "album" => Ok(Field::Album),
            "mp3_year" => Ok(Field::Year),
            "mp3_genre" | "genre" => Ok(Field::Genre),
            "exif_altitude" | "exif_alt" => Ok(Field::ExifGpsAltitude),
            "exif_datetime" => Ok(Field::ExifDateTime),
            "exif_latitude" | "exif_lat" => Ok(Field::ExifGpsLatitude),
            "exif_longitude" | "exif_lon" | "exif_lng" => Ok(Field::ExifGpsLongitude),
            "exif_make" => Ok(Field::ExifMake),
            "exif_model" => Ok(Field::ExifModel),
            "exif_software" => Ok(Field::ExifSoftware),
            "exif_version" => Ok(Field::ExifVersion),
            "is_binary" => Ok(Field::IsBinary),
            "is_text" => Ok(Field::IsText),
            "is_archive" => Ok(Field::IsArchive),
            "is_audio" => Ok(Field::IsAudio),
            "is_book" => Ok(Field::IsBook),
            "is_doc" => Ok(Field::IsDoc),
            "is_font" => Ok(Field::IsFont),
            "is_image" => Ok(Field::IsImage),
            "is_source" => Ok(Field::IsSource),
            "is_video" => Ok(Field::IsVideo),
            "sha1" => Ok(Field::Sha1),
            "sha2_256" | "sha256" => Ok(Field::Sha256),
            "sha2_512" | "sha512" => Ok(Field::Sha512),
            "sha3_512" | "sha3" => Ok(Field::Sha3),
            _ => {
                let err = String::from("Unknown field ") + &field;
                Err(err)
            }
        }
    }
}

impl Display for Field {
    fn fmt(&self, f: &mut Formatter) -> Result<(), Error> {
        write!(f, "{:?}", self)
    }
}

impl Serialize for Field {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl Field {
    #[rustfmt::skip]
    pub fn is_numeric_field(&self) -> bool {
        matches!(self, Field::Size | Field::FormattedSize
            | Field::Uid | Field::Gid
            | Field::Width | Field::Height
            | Field::LineCount
            | Field::Duration
            | Field::Bitrate | Field::Freq | Field::Year
            | Field::ExifGpsLatitude | Field::ExifGpsLongitude | Field::ExifGpsAltitude)
    }

    pub fn is_datetime_field(&self) -> bool {
        matches!(
            self,
            Field::Created | Field::Accessed | Field::Modified | Field::ExifDateTime
        )
    }

    pub fn is_boolean_field(&self) -> bool {
        matches!(
            self,
            Field::IsDir
                | Field::IsFile
                | Field::UserRead
                | Field::UserWrite
                | Field::UserExec
                | Field::UserAll
                | Field::GroupRead
                | Field::GroupWrite
                | Field::GroupExec
                | Field::GroupAll
                | Field::OtherRead
                | Field::OtherWrite
                | Field::OtherExec
                | Field::OtherAll
                | Field::Suid
                | Field::Sgid
                | Field::IsSymlink
                | Field::IsPipe
                | Field::IsCharacterDevice
                | Field::IsBlockDevice
                | Field::IsSocket
                | Field::IsHidden
                | Field::HasXattrs
                | Field::IsEmpty
                | Field::IsShebang
                | Field::IsBinary
                | Field::IsText
                | Field::IsArchive
                | Field::IsAudio
                | Field::IsBook
                | Field::IsDoc
                | Field::IsFont
                | Field::IsImage
                | Field::IsSource
                | Field::IsVideo
        )
    }

    pub fn is_available_for_archived_files(&self) -> bool {
        matches!(
            self,
            Field::Name
                | Field::Extension
                | Field::Path
                | Field::AbsPath
                | Field::Directory
                | Field::AbsDir
                | Field::Size
                | Field::FormattedSize
                | Field::IsDir
                | Field::IsFile
                | Field::IsSymlink
                | Field::IsPipe
                | Field::IsCharacterDevice
                | Field::IsBlockDevice
                | Field::IsSocket
                | Field::Mode
                | Field::UserRead
                | Field::UserWrite
                | Field::UserExec
                | Field::UserAll
                | Field::GroupRead
                | Field::GroupWrite
                | Field::GroupExec
                | Field::GroupAll
                | Field::OtherRead
                | Field::OtherWrite
                | Field::OtherExec
                | Field::OtherAll
                | Field::Suid
                | Field::Sgid
                | Field::IsHidden
                | Field::IsEmpty
                | Field::Modified
                | Field::IsArchive
                | Field::IsAudio
                | Field::IsBook
                | Field::IsDoc
                | Field::IsFont
                | Field::IsImage
                | Field::IsSource
                | Field::IsVideo
        )
    }

    pub fn is_colorized_field(&self) -> bool {
        matches!(self, Field::Name)
    }
}
