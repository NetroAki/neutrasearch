# Live User Feedback

Use this file to record hands-on feedback exactly as it is reported.

## Instructions

- Record each observation quickly and faithfully.
- Include the date/time and relevant OS or screen when known.
- Do not investigate, fix, reinterpret, or expand feedback while it is being collected.
- Wait until the user explicitly asks for review or implementation.

## Entries

### 2026-07-20 — Linux/Wayland hands-on feedback

- The app icon is not the Neutrasearch logo.
- The top-left area of the app is not using the Neutrasearch logo.
- Green is not the desired visual direction. Use more sullen colors, potentially borrowing inspiration from Termius.
- The initial **Choose where to search** window has unnecessary icons and too much information.
- Remove the reassurance copy about filenames staying on the computer, not uploading the index, and not using telemetry from this window.
- The initial window should be very simple:
  - Press a plus button to open the file browser.
  - Select a folder and return to the window.
  - Allow any number of folders to be added or removed.
  - Press one button to scan.
  - Never show this initial screen again after setup.
  - Additional locations should later be managed from the settings menu.
- Resizing/scaling the whole application window feels laggy and overly smoothed; it does not follow the pointer quickly enough.
- The outer window appears as a generic Wayland window and shows the Wayland logo in the top-left instead of the Neutrasearch logo.
- Reduce the toolbar to at most three options, plus the additional Help option.
- The Help menu is missing the donation links. The rest of that menu is fine, aside from the missing logo.
- Resizing panes/windows inside the app feels fine. The lag appears limited to resizing the Wayland application window itself.
- Enabling network helpers causes many errors, mainly because some mounted machines are currently offline. Ignore offline machines and index them when they come online.
- Indexing with elevated/sudo access should be possible directly within the application, potentially as part of first installation.
- Text cannot be copied and pasted within the window.
- Permission-denied directories appear to block scanning. Ignore those directories and continue indexing instead.
