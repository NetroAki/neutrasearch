use crate::bootstrap::app_icon;

#[test]
fn embedded_logo_decodes_to_a_complete_rgba_icon() {
    let icon = app_icon();
    assert_eq!((icon.width, icon.height), (128, 128));
    assert_eq!(icon.rgba.len(), 128 * 128 * 4);
}
