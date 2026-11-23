//! Application menu policy, composed into the shell and CEF adapters at startup.

use jfn_platform_abi::{
    MenuDelivery, MenuItem, MenuKind, MenuOpenError, MenuRequest, MenuSelection, WindowPoint,
};

// CEF's custom command range. Shell edit menus use their own request-local IDs.
const TOGGLE_FULLSCREEN: i32 = 26_500;
const CLIENT_SETTINGS: i32 = TOGGLE_FULLSCREEN + 1;
const EXIT: i32 = TOGGLE_FULLSCREEN + 2;
const OPEN_FILE: i32 = TOGGLE_FULLSCREEN + 3;
const DOWNLOADS: i32 = TOGGLE_FULLSCREEN + 4;

fn items(restricted: bool) -> Vec<MenuItem> {
    [
        (TOGGLE_FULLSCREEN, "Toggle Fullscreen"),
        (OPEN_FILE, "Open File…"),
        (DOWNLOADS, "Downloads"),
        (CLIENT_SETTINGS, "Settings"),
        (EXIT, "Exit"),
    ]
    .into_iter()
    .filter(|(id, _)| !restricted || *id != CLIENT_SETTINGS)
    .map(|(id, label)| MenuItem {
        id,
        label: label.to_owned(),
        enabled: true,
        separator: false,
    })
    .collect()
}

fn dispatch(id: i32) -> bool {
    match id {
        TOGGLE_FULLSCREEN => {
            if let Some(platform) = jfn_platform_abi::try_lease() {
                platform.toggle_fullscreen();
            }
        }
        OPEN_FILE => crate::local_video::pick(),
        DOWNLOADS => crate::shell::post(crate::shell::actor::Work::OpenDownloads),
        CLIENT_SETTINGS => crate::shell::shell_open_client_settings(),
        EXIT => jfn_playback::shutdown::jfn_shutdown_initiate(),
        _ => return false,
    }
    true
}

fn open_host(
    anchor: WindowPoint,
    basis: jfn_platform_abi::WindowBasis,
    restricted: bool,
) -> Result<(), MenuOpenError> {
    let Some(platform) = jfn_platform_abi::try_lease() else {
        return Ok(());
    };
    let MenuDelivery::Host(host) = platform.menu_delivery(MenuKind::ContextMenu) else {
        return Ok(());
    };
    host.open(MenuRequest {
        items: items(restricted),
        anchor,
        basis,
        width: 0,
        initial: jfn_platform_abi::MENU_DISMISSED,
        on_selected: MenuSelection::new(|id| {
            dispatch(id);
        }),
    })
    .map(drop)
}

pub(crate) fn shell_actions() -> crate::shell::ApplicationActions {
    crate::shell::ApplicationActions {
        open_menu: open_host,
    }
}
pub(crate) fn cef_menu() -> jfn_cef::ApplicationMenu {
    jfn_cef::ApplicationMenu {
        items: items(false),
        on_selected: std::sync::Arc::new(dispatch),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restricted_menu_keeps_fullscreen_and_exit_but_omits_settings() {
        assert_eq!(
            items(false).iter().map(|item| item.id).collect::<Vec<_>>(),
            [
                TOGGLE_FULLSCREEN,
                OPEN_FILE,
                DOWNLOADS,
                CLIENT_SETTINGS,
                EXIT
            ]
        );
        assert_eq!(
            items(true).iter().map(|item| item.id).collect::<Vec<_>>(),
            [TOGGLE_FULLSCREEN, OPEN_FILE, DOWNLOADS, EXIT]
        );
        assert!(!dispatch(jfn_platform_abi::MENU_DISMISSED));
    }
}
