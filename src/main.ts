import { getCurrentWindow } from "@tauri-apps/api/window";

// The popup is destroyed on blur by the Rust side; Escape is the keyboard
// equivalent.
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape") {
    void getCurrentWindow().close();
  }
});
