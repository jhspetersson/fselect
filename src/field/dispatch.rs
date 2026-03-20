use crate::field::Field;
use crate::field::context::FieldContext;
use crate::field::{
    content_handlers, exif_handlers, hash_handlers, media_handlers, metadata_handlers,
    mode_handlers, path_handlers,
};
use crate::util::*;
use crate::util::error::SearchError;

pub fn get_field_value(ctx: &mut FieldContext, field: &Field) -> Result<Variant, SearchError> {
    if ctx.file_info.is_some() && !field.is_available_for_archived_files() {
        return Ok(Variant::empty(VariantType::String));
    }

    match field {
        // Path fields
        Field::Name => path_handlers::handle_name(ctx),
        Field::Filename => path_handlers::handle_filename(ctx),
        Field::Extension => path_handlers::handle_extension(ctx),
        Field::Path => path_handlers::handle_path(ctx),
        Field::AbsPath => path_handlers::handle_abspath(ctx),
        Field::Directory => path_handlers::handle_directory(ctx),
        Field::AbsDir => path_handlers::handle_absdir(ctx),

        // Size / type metadata
        Field::Size => metadata_handlers::handle_size(ctx),
        Field::FormattedSize => metadata_handlers::handle_formatted_size(ctx),
        Field::IsDir => metadata_handlers::handle_is_dir(ctx),
        Field::IsFile => metadata_handlers::handle_is_file(ctx),
        Field::IsSymlink => metadata_handlers::handle_is_symlink(ctx),
        Field::IsPipe => metadata_handlers::handle_is_pipe(ctx),
        Field::IsCharacterDevice => metadata_handlers::handle_is_char_device(ctx),
        Field::IsBlockDevice => metadata_handlers::handle_is_block_device(ctx),
        Field::IsSocket => metadata_handlers::handle_is_socket(ctx),
        Field::Device => metadata_handlers::handle_device(ctx),
        Field::Rdev => metadata_handlers::handle_rdev(ctx),
        Field::Inode => metadata_handlers::handle_inode(ctx),
        Field::Blocks => metadata_handlers::handle_blocks(ctx),
        Field::Hardlinks => metadata_handlers::handle_hardlinks(ctx),
        Field::Atime => metadata_handlers::handle_atime(ctx),
        Field::Mtime => metadata_handlers::handle_mtime(ctx),
        Field::Ctime => metadata_handlers::handle_ctime(ctx),
        Field::Created => metadata_handlers::handle_created(ctx),
        Field::Accessed => metadata_handlers::handle_accessed(ctx),
        Field::Modified => metadata_handlers::handle_modified(ctx),
        Field::IsHidden => metadata_handlers::handle_is_hidden(ctx),
        Field::IsEmpty => metadata_handlers::handle_is_empty(ctx),
        Field::HasXattrs => metadata_handlers::handle_has_xattrs(ctx),
        Field::XattrCount => metadata_handlers::handle_xattr_count(ctx),
        Field::Extattrs => metadata_handlers::handle_extattrs(ctx),
        Field::HasExtattrs => metadata_handlers::handle_has_extattrs(ctx),
        Field::Acl => metadata_handlers::handle_acl(ctx),
        Field::HasAcl => metadata_handlers::handle_has_acl(ctx),
        Field::DefaultAcl => metadata_handlers::handle_default_acl(ctx),
        Field::HasDefaultAcl => metadata_handlers::handle_has_default_acl(ctx),
        Field::HasCapabilities => metadata_handlers::handle_has_capabilities(ctx),
        Field::Capabilities => metadata_handlers::handle_capabilities(ctx),
        Field::IsShebang => metadata_handlers::handle_is_shebang(ctx),

        // Mode / permissions
        Field::Mode => mode_handlers::handle_mode(ctx),
        Field::UserRead => mode_handlers::handle_user_read(ctx),
        Field::UserWrite => mode_handlers::handle_user_write(ctx),
        Field::UserExec => mode_handlers::handle_user_exec(ctx),
        Field::UserAll => mode_handlers::handle_user_all(ctx),
        Field::GroupRead => mode_handlers::handle_group_read(ctx),
        Field::GroupWrite => mode_handlers::handle_group_write(ctx),
        Field::GroupExec => mode_handlers::handle_group_exec(ctx),
        Field::GroupAll => mode_handlers::handle_group_all(ctx),
        Field::OtherRead => mode_handlers::handle_other_read(ctx),
        Field::OtherWrite => mode_handlers::handle_other_write(ctx),
        Field::OtherExec => mode_handlers::handle_other_exec(ctx),
        Field::OtherAll => mode_handlers::handle_other_all(ctx),
        Field::Suid => mode_handlers::handle_suid(ctx),
        Field::Sgid => mode_handlers::handle_sgid(ctx),
        Field::IsSticky => mode_handlers::handle_is_sticky(ctx),
        Field::Uid => mode_handlers::handle_uid(ctx),
        Field::Gid => mode_handlers::handle_gid(ctx),
        #[cfg(all(unix, feature = "users"))]
        Field::User => mode_handlers::handle_user(ctx),
        #[cfg(all(unix, feature = "users"))]
        Field::Group => mode_handlers::handle_group(ctx),

        // Media (dimensions, audio)
        Field::Width => media_handlers::handle_width(ctx),
        Field::Height => media_handlers::handle_height(ctx),
        Field::Duration => media_handlers::handle_duration(ctx),
        Field::Bitrate => media_handlers::handle_bitrate(ctx),
        Field::Freq => media_handlers::handle_freq(ctx),
        Field::Title => media_handlers::handle_title(ctx),
        Field::Artist => media_handlers::handle_artist(ctx),
        Field::Album => media_handlers::handle_album(ctx),
        Field::Year => media_handlers::handle_year(ctx),
        Field::Genre => media_handlers::handle_genre(ctx),

        // EXIF
        Field::ExifDateTime | Field::ExifDateTimeOriginal => {
            exif_handlers::handle_exif_datetime(ctx, field)
        }
        Field::ExifGpsAltitude => exif_handlers::handle_exif_gps_altitude(ctx),
        Field::ExifGpsLatitude => exif_handlers::handle_exif_gps_latitude(ctx),
        Field::ExifGpsLongitude => exif_handlers::handle_exif_gps_longitude(ctx),
        Field::ExifMake
        | Field::ExifModel
        | Field::ExifSoftware
        | Field::ExifVersion
        | Field::ExifExposureTime
        | Field::ExifAperture
        | Field::ExifShutterSpeed
        | Field::ExifFNumber
        | Field::ExifIsoSpeed
        | Field::ExifPhotographicSensitivity
        | Field::ExifFocalLength
        | Field::ExifLensMake
        | Field::ExifLensModel
        | Field::ExifDescription
        | Field::ExifArtist
        | Field::ExifCopyright
        | Field::ExifOrientation
        | Field::ExifFlash
        | Field::ExifColorSpace
        | Field::ExifExposureProgram
        | Field::ExifExposureBias
        | Field::ExifWhiteBalance
        | Field::ExifMeteringMode
        | Field::ExifSceneType
        | Field::ExifContrast
        | Field::ExifSaturation
        | Field::ExifSharpness
        | Field::ExifBodySerial
        | Field::ExifLensSerial
        | Field::ExifUserComment
        | Field::ExifImageWidth
        | Field::ExifImageHeight
        | Field::ExifMaxAperture
        | Field::ExifDigitalZoom => exif_handlers::handle_exif_string(ctx, field),

        // Content
        Field::LineCount => content_handlers::handle_line_count(ctx),
        Field::Mime => content_handlers::handle_mime(ctx),
        Field::IsBinary | Field::IsText => content_handlers::handle_is_binary_or_text(ctx, field),
        Field::IsArchive
        | Field::IsAudio
        | Field::IsBook
        | Field::IsDoc
        | Field::IsFont
        | Field::IsImage
        | Field::IsSource
        | Field::IsVideo => content_handlers::handle_is_type(ctx, field),

        // Hashes
        Field::Sha1 => hash_handlers::handle_sha1(ctx),
        Field::Sha256 => hash_handlers::handle_sha256(ctx),
        Field::Sha512 => hash_handlers::handle_sha512(ctx),
        Field::Sha3 => hash_handlers::handle_sha3(ctx),
    }
}
