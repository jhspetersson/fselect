extern crate serde;

use std::str::FromStr;

use serde::ser::{Serialize, Serializer};
use std::fmt::Display;
use std::fmt::Formatter;
use std::fmt::Error;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Hash)]
pub enum Field {
    Name,
    Path,
    Size,
    FormattedSize,
    Uid,
    Gid,
    User,
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
    Mode,
    UserRead,
    UserWrite,
    UserExec,
    GroupRead,
    GroupWrite,
    GroupExec,
    OtherRead,
    OtherWrite,
    OtherExec,
    IsHidden,
    HasXattrs,
    IsShebang,
    Width,
    Height,
    Bitrate,
    Freq,
    Title,
    Artist,
    Album,
    Year,
    Genre,
    ExifMake,
    ExifModel,
    IsArchive,
    IsAudio,
    IsBook,
    IsDoc,
    IsImage,
    IsSource,
    IsVideo,
}

impl FromStr for Field {
    type Err = String;

    fn from_str<'a>(s: &str) -> Result<Self, Self::Err> {
        let field = s.to_ascii_lowercase();

        match field.as_str() {
            "name" => Ok(Field::Name),
            "path" => Ok(Field::Path),
            "size" => Ok(Field::Size),
            "fsize" | "hsize" => Ok(Field::FormattedSize),
            "uid" => Ok(Field::Uid),
            "gid" => Ok(Field::Gid),
            "user" => Ok(Field::User),
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
            "mode" => Ok(Field::Mode),
            "user_read" => Ok(Field::UserRead),
            "user_write" => Ok(Field::UserWrite),
            "user_exec" => Ok(Field::UserExec),
            "group_read" => Ok(Field::GroupRead),
            "group_write" => Ok(Field::GroupWrite),
            "group_exec" => Ok(Field::GroupExec),
            "other_read" => Ok(Field::OtherRead),
            "other_write" => Ok(Field::OtherWrite),
            "other_exec" => Ok(Field::OtherExec),
            "is_hidden" => Ok(Field::IsHidden),
            "has_xattrs" => Ok(Field::HasXattrs),
            "is_shebang" => Ok(Field::IsShebang),
            "width" => Ok(Field::Width),
            "height" => Ok(Field::Height),
            "mp3_bitrate" | "bitrate" => Ok(Field::Bitrate),
            "mp3_freq" | "freq" => Ok(Field::Freq),
            "mp3_title" | "title" => Ok(Field::Title),
            "mp3_artist" | "artist" => Ok(Field::Artist),
            "mp3_album" | "album" => Ok(Field::Album),
            "mp3_year" => Ok(Field::Year),
            "mp3_genre" | "genre" => Ok(Field::Genre),
            "exif_make" => Ok(Field::ExifMake),
            "exif_model" => Ok(Field::ExifModel),
            "is_archive" => Ok(Field::IsArchive),
            "is_audio" => Ok(Field::IsAudio),
            "is_book" => Ok(Field::IsBook),
            "is_doc" => Ok(Field::IsDoc),
            "is_image" => Ok(Field::IsImage),
            "is_source" => Ok(Field::IsSource),
            "is_video" => Ok(Field::IsVideo),
            _ => {
                let err = String::from("Unknown field ") + &field;
                Err(err)
            }
        }
    }
}

impl Display for Field {
    fn fmt(&self, f: &mut Formatter) -> Result<(), Error>{
        write!(f, "{:?}", self)
    }
}

impl Serialize for Field {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where S: Serializer
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl Field {
    pub fn is_numeric_field(&self) -> bool {
        match self {
            Field::Size | Field::FormattedSize
            | Field::Uid | Field::Gid
            | Field::Width | Field::Height
            | Field::Bitrate | Field::Freq | Field::Year => true,
            _ => false
        }
    }

    pub fn is_datetime_field(&self) -> bool {
        match self {
            Field::Created | Field::Accessed | Field::Modified => true,
            _ => false
        }
    }

    pub fn is_mp3_field(&self) -> bool {
        match self {
            Field::Bitrate | Field::Freq | Field::Title
            | Field::Artist | Field::Album | Field::Year | Field::Genre => true,
            _ => false
        }
    }

    pub fn is_exif_field(&self) -> bool {
        match self {
            Field::ExifMake | Field::ExifModel => true,
            _ => false
        }
    }
}