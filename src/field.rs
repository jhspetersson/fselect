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
        Name,
        
        #[text = ["path"]]
        @for_archived = true
        Path,
        
        #[text = ["abspath"]]
        @for_archived = true
        @weight = 1
        AbsPath,
        
        #[text = ["ext", "extension"]]
        @for_archived = true
        Extension,
        
        #[text = ["dir", "directory", "dirname"]]
        @for_archived = true
        Directory,
        
        #[text = ["absdir"]]
        @for_archived = true
        @weight = 1
        AbsDir,
        
        #[text = ["size"], data_type = "numeric"]
        @for_archived = true
        @weight = 1
        Size,
        
        #[text = ["fsize", "hsize"], data_type = "numeric"]
        @for_archived = true
        @weight = 1
        FormattedSize,
        
        #[text = ["uid"], data_type = "numeric"]
        @weight = 1
        Uid,
        
        #[text = ["gid"], data_type = "numeric"]
        @weight = 1
        Gid,
        
        #[text = ["user"]]
        @weight = 1
        #[cfg(all(unix, feature = "users"))]
        User,
        
        #[text = ["group"]]
        @weight = 1
        #[cfg(all(unix, feature = "users"))]
        Group,
        
        #[text = ["created"], data_type = "datetime"]
        @weight = 1
        Created,
        
        #[text = ["accessed"], data_type = "datetime"]
        @weight = 1
        Accessed,
        
        #[text = ["modified"], data_type = "datetime"]
        @for_archived = true
        @weight = 1
        Modified,
        
        #[text = ["is_dir"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        IsDir,
        
        #[text = ["is_file"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        IsFile,
        
        #[text = ["is_symlink"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        IsSymlink,
        
        #[text = ["is_pipe", "is_fifo"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        IsPipe,
        
        #[text = ["is_char", "is_character"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        IsCharacterDevice,
        
        #[text = ["is_block"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        IsBlockDevice,
        
        #[text = ["is_socket"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        IsSocket,
        
        #[text = ["device"]]
        @weight = 1
        Device,
        
        #[text = ["inode"]]
        @weight = 1
        Inode,
        
        #[text = ["blocks"]]
        @weight = 1
        Blocks,
        
        #[text = ["hardlinks"]]
        @weight = 1
        Hardlinks,
        
        #[text = ["mode"]]
        @for_archived = true
        @weight = 1
        Mode,
        
        #[text = ["user_read"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        UserRead,
        
        #[text = ["user_write"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        UserWrite,
        
        #[text = ["user_exec"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        UserExec,
        
        #[text = ["user_all", "user_rwx"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        UserAll,
        
        #[text = ["group_read"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        GroupRead,
        
        #[text = ["group_write"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        GroupWrite,
        
        #[text = ["group_exec"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        GroupExec,
        
        #[text = ["group_all", "group_rwx"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        GroupAll,
        
        #[text = ["other_read"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        OtherRead,
        
        #[text = ["other_write"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        OtherWrite,
        
        #[text = ["other_exec"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        OtherExec,
        
        #[text = ["other_all", "other_rwx"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        OtherAll,
        
        #[text = ["suid"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        Suid,
        
        #[text = ["sgid"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        Sgid,
        
        #[text = ["is_hidden"], data_type = "boolean"]
        @for_archived = true
        @weight = 1
        IsHidden,
        
        #[text = ["has_xattrs"], data_type = "boolean"]
        @weight = 2
        HasXattrs,
        
        #[text = ["capabilities", "caps"]]
        @weight = 2
        Capabilities,
        
        #[text = ["is_shebang"], data_type = "boolean"]
        @weight = 2
        IsShebang,
        
        #[text = ["is_empty"], data_type = "boolean"]
        @for_archived = true
        @weight = 2
        IsEmpty,
        
        #[text = ["width"], data_type = "numeric"]
        @weight = 16
        Width,
        
        #[text = ["height"], data_type = "numeric"]
        @weight = 16
        Height,
        
        #[text = ["duration"], data_type = "numeric"]
        @weight = 16
        Duration,
        
        #[text = ["mp3_bitrate", "bitrate"], data_type = "numeric"]
        @weight = 16
        Bitrate,
        
        #[text = ["mp3_freq", "freq"], data_type = "numeric"]
        @weight = 16
        Freq,
        
        #[text = ["mp3_title", "title"]]
        @weight = 16
        Title,
        
        #[text = ["mp3_artist", "artist"]]
        @weight = 16
        Artist,
        
        #[text = ["mp3_album", "album"]]
        @weight = 16
        Album,
        
        #[text = ["mp3_year"], data_type = "numeric"]
        @weight = 16
        Year,
        
        #[text = ["mp3_genre", "genre"]]
        @weight = 16
        Genre,
        
        #[text = ["exif_datetime"], data_type = "datetime"]
        @weight = 16
        ExifDateTime,
        
        #[text = ["exif_altitude", "exif_alt"], data_type = "numeric"]
        @weight = 16
        ExifGpsAltitude,
        
        #[text = ["exif_latitude", "exif_lat"], data_type = "numeric"]
        @weight = 16
        ExifGpsLatitude,
        
        #[text = ["exif_longitude", "exif_lon", "exif_lng"], data_type = "numeric"]
        @weight = 16
        ExifGpsLongitude,
        
        #[text = ["exif_make"]]
        @weight = 16
        ExifMake,
        
        #[text = ["exif_model"]]
        @weight = 16
        ExifModel,
        
        #[text = ["exif_software"]]
        @weight = 16
        ExifSoftware,
        
        #[text = ["exif_version"]]
        @weight = 16
        ExifVersion,
        
        #[text = ["exif_exposure_time", "exif_exptime"], data_type = "numeric"]
        @weight = 16
        ExifExposureTime,
        
        #[text = ["exif_aperture"], data_type = "numeric"]
        @weight = 16
        ExifAperture,
        
        #[text = ["exif_shutter_speed"], data_type = "numeric"]
        @weight = 16
        ExifShutterSpeed,
        
        #[text = ["exif_f_number", "exif_f_num"], data_type = "numeric"]
        @weight = 16
        ExifFNumber,
        
        #[text = ["exif_iso_speed", "exif_iso"]]
        @weight = 16
        ExifIsoSpeed,
        
        #[text = ["exif_focal_length", "exif_focal_len"], data_type = "numeric"]
        @weight = 16
        ExifFocalLength,
        
        #[text = ["exif_lens_make"]]
        @weight = 16
        ExifLensMake,
        
        #[text = ["exif_lens_model"]]
        @weight = 16
        ExifLensModel,
        
        #[text = ["mime"]]
        @weight = 16
        Mime,
        
        #[text = ["line_count"], data_type = "numeric"]
        @weight = 1024
        LineCount,
        
        #[text = ["is_binary"], data_type = "boolean"]
        @weight = 16
        IsBinary,
        
        #[text = ["is_text"], data_type = "boolean"]
        @weight = 16
        IsText,
        
        #[text = ["is_archive"], data_type = "boolean"]
        @for_archived = true
        IsArchive,
        
        #[text = ["is_audio"], data_type = "boolean"]
        @for_archived = true
        IsAudio,
        
        #[text = ["is_book"], data_type = "boolean"]
        @for_archived = true
        IsBook,
        
        #[text = ["is_doc"], data_type = "boolean"]
        @for_archived = true
        IsDoc,
        
        #[text = ["is_font"], data_type = "boolean"]
        @for_archived = true
        IsFont,
        
        #[text = ["is_image"], data_type = "boolean"]
        @for_archived = true
        IsImage,
        
        #[text = ["is_source"], data_type = "boolean"]
        @for_archived = true
        IsSource,
        
        #[text = ["is_video"], data_type = "boolean"]
        @for_archived = true
        IsVideo,
        
        #[text = ["sha1"]]
        @weight = 1024
        Sha1,
        
        #[text = ["sha2_256", "sha256"]]
        @weight = 1024
        Sha256,
        
        #[text = ["sha2_512", "sha512"]]
        @weight = 1024
        Sha512,
        
        #[text = ["sha3_512", "sha3"]]
        @weight = 1024
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