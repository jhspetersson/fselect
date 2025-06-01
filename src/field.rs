//! Defines the various fields available in the query language

use std::fmt::Display;
use std::fmt::Error;
use std::fmt::Formatter;
use std::str::FromStr;

use serde::ser::{Serialize, Serializer};

macro_rules! fields {
    (
        $(#[$enum_attrs:meta])*
        $vis:vis enum $enum_name:ident {
            $(
                #[text = [$($text:literal),*]$(,)? $(data_type = $data_type:literal)?]
                $(@colorized = $colorized:literal)?
                $(@for_archived = $for_archived:literal)?
                $(@weight = $weight:literal)?
                $(@description = $description:literal)?
                $(#[$variant_attrs:meta])*
                $variant:ident
            ),*
            $(,)?
        }
        
    ) => {
        $(#[$enum_attrs])*
        $vis enum $enum_name {
            $(
                $(#[$variant_attrs])*
                $variant,
            )*
        }
        
        impl FromStr for $enum_name {
            type Err = String;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                let field = s.to_ascii_lowercase();

                match field.as_str() {
                    $(
                        $(#[$variant_attrs])*
                        $($text)|* => Ok($enum_name::$variant),
                    )*
                    _ => {
                        let err = String::from("Unknown field ") + &field;
                        Err(err)
                    }
                }
            }
        }
        
        impl Display for $enum_name {
           fn fmt(&self, f: &mut Formatter) -> Result<(), Error> {
                write!(f, "{:?}", self)
            }
        }

        impl Serialize for $enum_name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.to_string())
            }
        }
        
        impl $enum_name {
            pub fn is_numeric_field(&self) -> bool {
                match self {
                    $(
                        $(#[$variant_attrs])*
                        $enum_name::$variant => {
                            stringify!($($data_type)?) .replace("\"", "") == "numeric"
                        }
                    )*
                }
            }
            
            pub fn is_datetime_field(&self) -> bool {
                match self {
                    $(
                        $(#[$variant_attrs])*
                        $enum_name::$variant => {
                            stringify!($($data_type)?) .replace("\"", "") == "datetime"
                        }
                    )*
                }
            }
            
            pub fn is_boolean_field(&self) -> bool {
                match self {
                    $(
                        $(#[$variant_attrs])*
                        $enum_name::$variant => {
                            stringify!($($data_type)?) .replace("\"", "") == "boolean"
                        }
                    )*
                }
            }
            
            pub fn is_colorized_field(&self) -> bool {
                match self {
                    $(
                        $(#[$variant_attrs])*
                        $enum_name::$variant => {
                            stringify!($($colorized)?) == "true"
                        }
                    )*
                }
            }
            
            pub fn is_available_for_archived_files(&self) -> bool {
                match self {
                    $(
                        $(#[$variant_attrs])*
                        $enum_name::$variant => {
                            stringify!($($for_archived)?) == "true"
                        }
                    )*
                }
            }
            
            pub fn get_weight(&self) -> i32 {
                match self {
                    $(
                        $(#[$variant_attrs])*
                        $enum_name::$variant => {
                            stringify!($($weight)?) .parse().unwrap_or(0)
                        }
                    )*
                }
            }
        }
    };
}

fields! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Hash)]
    pub enum Field {
        #[text = ["name"]]
        @colorized = true
        @for_archived = true
        @description = "Returns the name (with extension) of the file"
        Name,
        
        #[text = ["ext", "extension"]]
        @for_archived = true
        @description = "Returns the extension of the file"
        Extension,
        
        #[text = ["path"]]
        @for_archived = true
        @description = "Returns the path of the file"
        Path,
        
        #[text = ["abspath"]]
        @for_archived = true
        @weight = 1
        @description = "Returns the absolute path of the file"
        AbsPath,
        
        #[text = ["dir", "directory", "dirname"]]
        @for_archived = true
        @description = "Returns the directory of the file"
        Directory,
        
        #[text = ["absdir"]]
        @for_archived = true
        @weight = 1
        @description = "Returns the absolute directory of the file"
        AbsDir,
        
        #[text = ["size"], data_type = "numeric"]
        @for_archived = true
        @weight = 1
        @description = "Returns the size of the file in bytes"
        Size,
        
        #[text = ["fsize", "hsize"], data_type = "numeric"]
        @for_archived = true
        @weight = 1
        @description = "Returns the size of the file accompanied with the unit"
        FormattedSize,
        
        #[text = ["uid"], data_type = "numeric"]
        @weight = 1
        @description = "Returns the UID of the owner"
        Uid,
        
        #[text = ["gid"], data_type = "numeric"]
        @weight = 1
        @description = "Returns the GID of the owner's group"
        Gid,
        
        #[text = ["user"]]
        @weight = 1
        @description = "Returns the name of the owner for this file"
        #[cfg(all(unix, feature = "users"))]
        User,
        
        #[text = ["group"]]
        @weight = 1
        @description = "Returns the name of the owner's group for this file"
        #[cfg(all(unix, feature = "users"))]
        Group,
        
        #[text = ["created"], data_type = "datetime"]
        @weight = 1
        @description = "Returns the file creation date (YYYY-MM-DD HH:MM:SS)"
        Created,
        
        #[text = ["accessed"], data_type = "datetime"]
        @weight = 1
        @description = "Returns the time the file was last accessed (YYYY-MM-DD HH:MM:SS)"
        Accessed,
        
        #[text = ["modified"], data_type = "datetime"]
        @for_archived = true
        @weight = 1
        @description = "Returns the time the file was last modified (YYYY-MM-DD HH:MM:SS)"
        Modified,
        
        #[text = ["is_dir"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        @description = "Returns a boolean signifying whether the file path is a directory"
        IsDir,
        
        #[text = ["is_file"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        @description = "Returns a boolean signifying whether the file path is a file"
        IsFile,
        
        #[text = ["is_symlink"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        @description = "Returns a boolean signifying whether the file path is a symlink"
        IsSymlink,
        
        #[text = ["is_pipe", "is_fifo"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        @description = "Returns a boolean signifying whether the file path is a FIFO or pipe file"
        IsPipe,
        
        #[text = ["is_char", "is_character"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        @description = "Returns a boolean signifying whether the file path is a character device or character special file"
        IsCharacterDevice,
        
        #[text = ["is_block"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        @description = "Returns a boolean signifying whether the file path is a block or block special file"
        IsBlockDevice,
        
        #[text = ["is_socket"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        @description = "Returns a boolean signifying whether the file path is a socket file"
        IsSocket,
        
        #[text = ["device"]]
        @weight = 1
        @description = "Returns the code of device the file is stored on"
        Device,
        
        #[text = ["inode"]]
        @weight = 1
        @description = "Returns the number of inode"
        Inode,
        
        #[text = ["blocks"]]
        @weight = 1
        @description = "Returns the number of blocks (256 bytes) the file occupies"
        Blocks,
        
        #[text = ["hardlinks"]]
        @weight = 1
        @description = "Returns the number of hardlinks of the file"
        Hardlinks,
        
        #[text = ["mode"]]
        @for_archived = true
        @weight = 1
        @description = "Returns the permissions of the owner, group, and everybody (similar to the first field in `ls -la`)"
        Mode,
        
        #[text = ["user_read"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        @description = "Returns a boolean signifying whether the file can be read by the owner"
        UserRead,
        
        #[text = ["user_write"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        @description = "Returns a boolean signifying whether the file can be written by the owner"
        UserWrite,
        
        #[text = ["user_exec"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        @description = "Returns a boolean signifying whether the file can be executed by the owner"
        UserExec,
        
        #[text = ["user_all", "user_rwx"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        @description = "Returns a boolean signifying whether the file can be fully accessed by the owner"
        UserAll,
        
        #[text = ["group_read"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        @description = "Returns a boolean signifying whether the file can be read by the owner's group"
        GroupRead,
        
        #[text = ["group_write"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        @description = "Returns a boolean signifying whether the file can be written by the owner's group"
        GroupWrite,
        
        #[text = ["group_exec"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        @description = "Returns a boolean signifying whether the file can be executed by the owner's group"
        GroupExec,
        
        #[text = ["group_all", "group_rwx"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        @description = "Returns a boolean signifying whether the file can be fully accessed by the group"
        GroupAll,
        
        #[text = ["other_read"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        @description = "Returns a boolean signifying whether the file can be read by others"
        OtherRead,
        
        #[text = ["other_write"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        @description = "Returns a boolean signifying whether the file can be written by others"
        OtherWrite,
        
        #[text = ["other_exec"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        @description = "Returns a boolean signifying whether the file can be executed by others"
        OtherExec,
        
        #[text = ["other_all", "other_rwx"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        @description = "Returns a boolean signifying whether the file can be fully accessed by the others"
        OtherAll,
        
        #[text = ["suid"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        @description = "Returns a boolean signifying whether the file permissions have a SUID bit set"
        Suid,
        
        #[text = ["sgid"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        @description = "Returns a boolean signifying whether the file permissions have a SGID bit set"
        Sgid,
        
        #[text = ["is_hidden"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        @description = "Returns a boolean signifying whether the file is a hidden file (e.g., files that start with a dot on *nix)"
        IsHidden,
        
        #[text = ["has_xattrs"], data_type = "boolean"]
        @weight = 2
        @description = "Returns a boolean signifying whether the file has extended attributes"
        HasXattrs,
        
        #[text = ["capabilities", "caps"]]
        @weight = 2
        @description = "Returns a string describing Linux capabilities assigned to a file"
        Capabilities,
        
        #[text = ["is_shebang"], data_type = "boolean"]
        @weight = 2
        @description = "Returns a boolean signifying whether the file starts with a shebang (#!)"
        IsShebang,
        
        #[text = ["is_empty"], data_type = "boolean"]
        @for_archived = true
        @weight = 2
        @description = "Returns a boolean signifying whether the file is empty or the directory is empty"
        IsEmpty,
        
        #[text = ["width"], data_type = "numeric"]
        @weight = 16
        @description = "Returns the number of pixels along the width of the photo or MP4 file"
        Width,
        
        #[text = ["height"], data_type = "numeric"]
        @weight = 16
        @description = "Returns the number of pixels along the height of the photo or MP4 file"
        Height,
        
        #[text = ["duration"], data_type = "numeric"]
        @weight = 16
        @description = "Returns the duration of audio file in seconds"
        Duration,
        
        #[text = ["mp3_bitrate", "bitrate"], data_type = "numeric"]
        @weight = 16
        @description = "Returns the bitrate of the audio file in kbps"
        Bitrate,
        
        #[text = ["mp3_freq", "freq"], data_type = "numeric"]
        @weight = 16
        @description = "Returns the sampling rate of audio or video file"
        Freq,
        
        #[text = ["mp3_title", "title"]]
        @weight = 16
        @description = "Returns the title of the audio file taken from the file's metadata"
        Title,
        
        #[text = ["mp3_artist", "artist"]]
        @weight = 16
        @description = "Returns the artist of the audio file taken from the file's metadata"
        Artist,
        
        #[text = ["mp3_album", "album"]]
        @weight = 16
        @description = "Returns the album name of the audio file taken from the file's metadata"
        Album,
        
        #[text = ["mp3_year"], data_type = "numeric"]
        @weight = 16
        @description = "Returns the year of the audio file taken from the file's metadata"
        Year,
        
        #[text = ["mp3_genre", "genre"]]
        @weight = 16
        @description = "Returns the genre of the audio file taken from the file's metadata"
        Genre,
        
        #[text = ["exif_datetime"], data_type = "datetime"]
        @weight = 16
        @description = "Returns date and time of taken photo"
        ExifDateTime,
        
        #[text = ["exif_altitude", "exif_alt"], data_type = "numeric"]
        @weight = 16
        @description = "Returns GPS altitude of taken photo"
        ExifGpsAltitude,
        
        #[text = ["exif_latitude", "exif_lat"], data_type = "numeric"]
        @weight = 16
        @description = "Returns GPS latitude of taken photo"
        ExifGpsLatitude,
        
        #[text = ["exif_longitude", "exif_lon", "exif_lng"], data_type = "numeric"]
        @weight = 16
        @description = "Returns GPS longitude of taken photo"
        ExifGpsLongitude,
        
        #[text = ["exif_make"]]
        @weight = 16
        @description = "Returns name of the camera manufacturer"
        ExifMake,
        
        #[text = ["exif_model"]]
        @weight = 16
        @description = "Returns camera model"
        ExifModel,
        
        #[text = ["exif_software"]]
        @weight = 16
        @description = "Returns software name with which the photo was taken"
        ExifSoftware,
        
        #[text = ["exif_version"]]
        @weight = 16
        @description = "Returns the version of EXIF metadata"
        ExifVersion,
        
        #[text = ["exif_exposure_time", "exif_exptime"], data_type = "numeric"]
        @weight = 16
        @description = "Returns exposure time of the photo taken"
        ExifExposureTime,
        
        #[text = ["exif_aperture"], data_type = "numeric"]
        @weight = 16
        @description = "Returns aperture value of the photo taken"
        ExifAperture,
        
        #[text = ["exif_shutter_speed"], data_type = "numeric"]
        @weight = 16
        @description = "Returns shutter speed of the photo taken"
        ExifShutterSpeed,
        
        #[text = ["exif_f_number", "exif_f_num"], data_type = "numeric"]
        @weight = 16
        @description = "Returns F-number of the photo taken"
        ExifFNumber,
        
        #[text = ["exif_iso_speed", "exif_iso"]]
        @weight = 16
        @description = "Returns ISO speed of the photo taken"
        ExifIsoSpeed,
        
        #[text = ["exif_focal_length", "exif_focal_len"], data_type = "numeric"]
        @weight = 16
        @description = "Returns focal length of the photo taken"
        ExifFocalLength,
        
        #[text = ["exif_lens_make"]]
        @weight = 16
        @description = "Returns lens manufacturer used to take the photo"
        ExifLensMake,
        
        #[text = ["exif_lens_model"]]
        @weight = 16
        @description = "Returns lens model used to take the photo"
        ExifLensModel,
        
        #[text = ["mime"]]
        @weight = 16
        @description = "Returns MIME type of the file"
        Mime,
        
        #[text = ["line_count"], data_type = "numeric"]
        @weight = 1024
        @description = "Returns a number of lines in a text file"
        LineCount,
        
        #[text = ["is_binary"], data_type = "boolean"]
        @weight = 16
        @description = "Returns a boolean signifying whether the file has binary contents"
        IsBinary,
        
        #[text = ["is_text"], data_type = "boolean"]
        @weight = 16
        @description = "Returns a boolean signifying whether the file has text contents"
        IsText,
        
        #[text = ["is_archive"], data_type = "boolean"]
        @for_archived = true
        @description = "Returns a boolean signifying whether the file is an archival file"
        IsArchive,
        
        #[text = ["is_audio"], data_type = "boolean"]
        @for_archived = true
        @description = "Returns a boolean signifying whether the file is an audio file"
        IsAudio,
        
        #[text = ["is_book"], data_type = "boolean"]
        @for_archived = true
        @description = "Returns a boolean signifying whether the file is a book"
        IsBook,
        
        #[text = ["is_doc"], data_type = "boolean"]
        @for_archived = true
        @description = "Returns a boolean signifying whether the file is a document"
        IsDoc,
        
        #[text = ["is_font"], data_type = "boolean"]
        @for_archived = true
        @description = "Returns a boolean signifying whether the file is a font"
        IsFont,
        
        #[text = ["is_image"], data_type = "boolean"]
        @for_archived = true
        @description = "Returns a boolean signifying whether the file is an image"
        IsImage,
        
        #[text = ["is_source"], data_type = "boolean"]
        @for_archived = true
        @description = "Returns a boolean signifying whether the file is source code"
        IsSource,
        
        #[text = ["is_video"], data_type = "boolean"]
        @for_archived = true
        @description = "Returns a boolean signifying whether the file is a video file"
        IsVideo,
        
        #[text = ["sha1"]]
        @weight = 1024
        @description = "Returns SHA-1 digest of a file"
        Sha1,
        
        #[text = ["sha2_256", "sha256"]]
        @weight = 1024
        @description = "Returns SHA2-256 digest of a file"
        Sha256,
        
        #[text = ["sha2_512", "sha512"]]
        @weight = 1024
        @description = "Returns SHA2-512 digest of a file"
        Sha512,
        
        #[text = ["sha3_512", "sha3"]]
        @weight = 1024
        @description = "Returns SHA-3 digest of a file"
        Sha3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_colorized() {
        let field = Field::Name;
        assert_eq!(field.is_colorized_field(), true);

        let field = Field::Size;
        assert_eq!(field.is_colorized_field(), false);
    }

    #[test]
    fn test_is_numeric_field() {
        let field = Field::Size;
        assert_eq!(field.is_numeric_field(), true);

        let field = Field::Name;
        assert_eq!(field.is_numeric_field(), false);
    }
    
    #[test]
    fn test_is_datetime_field() {
        let field = Field::Created;
        assert_eq!(field.is_datetime_field(), true);

        let field = Field::Name;
        assert_eq!(field.is_datetime_field(), false);
    }
    
    #[test]
    fn test_is_boolean_field() {
        let field = Field::IsDir;
        assert_eq!(field.is_boolean_field(), true);

        let field = Field::Name;
        assert_eq!(field.is_boolean_field(), false);
    }
    
    #[test]
    fn test_is_available_for_archived_files() {
        let field = Field::Name;
        assert_eq!(field.is_available_for_archived_files(), true);

        let field = Field::LineCount;
        assert_eq!(field.is_available_for_archived_files(), false);
    }

    #[test]
    fn test_weight() {
        let field = Field::Name;
        assert_eq!(field.get_weight(), 0);

        let field = Field::Size;
        assert_eq!(field.get_weight(), 1);
    }
}