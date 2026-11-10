// Opens kiosk on one of its tabs, for a key bound in the app: `mod.open:kiosk-<tab>`.
//
// The app can't open a mod with props, so this launcher, a 1×1 popup that draws nothing,
// does it through shared state that kiosk's main.js reads (see LAUNCH there):
// - kiosk closed: set LAUNCH to the tab, open kiosk, and close this popup;
// - kiosk open (OPEN is set): send it `kiosk.tab:<tab>:<nonce>`, which shows the tab, or closes
//   kiosk if it already shows. Kiosk acknowledges by setting ACK to the nonce. With no answer
//   in ACK_MS, OPEN is left over from a kiosk that crashed, so this clears it and opens kiosk.
// This popup takes the keyboard from an open kiosk, which takes it back.
//
// Every launcher's main.js is this file with its own TAB.

import { View, div } from "gpui-kit";
import { closeOverlay, props, run, setShared, shared } from "herdr";

const TAB = "diff";
const LAUNCH = "kiosk.launch";
const OPEN = "kiosk.open";
const ACK = "kiosk.ack";
// Kiosk answered within 50 ms in the sandbox.
const ACK_MS = 150;
const POLL_MS = 10;

export default class KioskLauncher extends View {
  init(_props, cx) {
    const overlay = props()?.overlay;
    // `herdr-app check` loads the mod with no overlay.
    if (!overlay) return;
    const openKiosk = () => {
      setShared(LAUNCH, TAB);
      run("mod.open", { name: "kiosk" });
      closeOverlay(overlay);
    };
    if (!shared(OPEN)) {
      openKiosk();
      return;
    }
    const sent = Date.now();
    const nonce = `${sent}-${Math.random()}`;
    run(`kiosk.tab:${TAB}:${nonce}`);
    const poll = cx.timer.every(POLL_MS, () => {
      const answered = shared(ACK) === nonce;
      if (!answered && Date.now() - sent < ACK_MS) return;
      poll.cancel();
      if (answered) {
        closeOverlay(overlay);
      } else {
        setShared(OPEN, null);
        openKiosk();
      }
    });
  }

  render() {
    return div();
  }
}
