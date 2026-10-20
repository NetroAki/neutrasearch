use super::*;

#[test]
fn subdirectory_mount_maps_only_paths_beneath_its_btrfs_root() {
    assert_eq!(mount_relative("/@/home", "@/home"), Some(String::new()));
    assert_eq!(
        mount_relative("/@/home", "@/home/user/docs"),
        Some("user/docs".into())
    );
    assert_eq!(mount_relative("/@/home", "@/homes/user"), None);
}

#[test]
fn subdirectory_handle_root_is_empty_and_descendants_are_relative() {
    // dir_path is already relative to the opened subvolume tree, not mountinfo's
    // filesystem root. A root handle must preserve ordinary child paths.
    assert_eq!(
        mount_relative("", "netroaki/docs"),
        Some("netroaki/docs".into())
    );
    assert_eq!(
        mount_relative("@home", "@home/netroaki"),
        Some("netroaki".into())
    );
}
