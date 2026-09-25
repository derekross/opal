import QtQuick
import qs.Commons
import qs.Ui

// First run: create or import the first account and choose the passphrase.
Column {
  id: root
  property var svc: null
  property color foreground: Color.foreground
  readonly property color dim: Qt.darker(foreground, 1.55)

  property string mode: "import"
  property bool busy: false
  property string error: ""
  readonly property bool isNcryptsec: secret.text.trim().indexOf("ncryptsec1") === 0

  spacing: Style.space(10)

  onVisibleChanged: if (!visible) {
    secret.text = ""; ncPass.text = ""; pass1.text = ""; pass2.text = ""; watchField.text = ""; error = ""
  }
  onModeChanged: { secret.text = ""; ncPass.text = ""; watchField.text = ""; error = "" }

  function submit() {
    error = ""
    if (mode === "watch") {
      var who = watchField.text.trim()
      if (who === "") { error = "Enter an npub or a NIP-05 address."; return }
      busy = true
      svc.call("identity.watch", { input: who }, function(err) {
        root.busy = false
        if (err) { root.error = err; return }
        watchField.text = ""
        root.svc.refreshConfig()
      })
      return
    }
    if (mode === "import" && secret.text.trim() === "") { error = "Paste your key first."; return }
    if (pass1.text.length < 10) { error = "Use at least 10 characters (a few random words work well)."; return }
    if (pass1.text !== pass2.text) { error = "The passphrases don't match."; return }
    busy = true
    var params = { passphrase: pass1.text, nickname: nickname.text }
    if (mode === "import") {
      params.secret = secret.text.trim()
      if (isNcryptsec) params.ncryptsec_password = ncPass.text
    }
    svc.call("accounts.add", params, function(err) {
      root.busy = false
      if (err) { root.error = err; return }
      secret.text = ""; ncPass.text = ""; pass1.text = ""; pass2.text = ""; nickname.text = ""
    })
  }

  Text {
    textFormat: Text.PlainText
    width: parent.width
    wrapMode: Text.Wrap
    color: root.foreground
    font.family: Style.font.family
    font.pixelSize: Style.font.body
    text: "Opal keeps your Nostr key in the system keyring, encrypted with a passphrase only you know. Apps log in with it over NIP-46 and never see the key."
  }

  ButtonGroup {
    width: parent.width
    options: [
      { value: "import", label: "Import key" },
      { value: "generate", label: "New key" },
      { value: "watch", label: "Watch someone" }
    ]
    value: root.mode
    foreground: root.foreground
    onChanged: function(v) { root.mode = v }
  }

  Text {
    textFormat: Text.PlainText
    width: parent.width
    visible: root.mode === "watch"
    wrapMode: Text.Wrap
    color: root.dim
    font.family: Style.font.family
    font.pixelSize: Style.font.bodySmall
    text: "Read-only: see notifications for anyone, no key needed. Enter an npub or a NIP-05 address like you@example.com. You can add your key later to sign and read DMs."
  }
  TextField {
    id: watchField
    width: parent.width
    visible: root.mode === "watch"
    placeholderText: "npub1… or name@domain"
    foreground: root.foreground
    onAccepted: root.submit()
  }

  TextField {
    id: secret
    width: parent.width
    visible: root.mode === "import"
    password: true
    placeholderText: "nsec, ncryptsec or recovery phrase"
    foreground: root.foreground
  }
  TextField {
    id: ncPass
    width: parent.width
    visible: root.mode === "import" && root.isNcryptsec
    password: true
    placeholderText: "Password of that ncryptsec"
    foreground: root.foreground
  }
  TextField {
    id: nickname
    width: parent.width
    visible: root.mode !== "watch"
    placeholderText: "Nickname (optional)"
    foreground: root.foreground
  }

  PanelSectionHeader {
    visible: root.mode !== "watch"
    text: "OPAL PASSPHRASE"
    foreground: root.dim
  }
  TextField {
    id: pass1
    visible: root.mode !== "watch"
    width: parent.width
    password: true
    placeholderText: "Passphrase (a few random words)"
    foreground: root.foreground
  }
  TextField {
    id: pass2
    visible: root.mode !== "watch"
    width: parent.width
    password: true
    placeholderText: "Repeat passphrase"
    foreground: root.foreground
    onAccepted: root.submit()
  }

  Text {
    textFormat: Text.PlainText
    width: parent.width
    visible: root.error !== ""
    wrapMode: Text.Wrap
    color: Color.urgent
    font.family: Style.font.family
    font.pixelSize: Style.font.bodySmall
    text: root.error
  }

  Button {
    text: root.busy ? (root.mode === "watch" ? "Looking up…" : "Encrypting…")
      : root.mode === "import" ? "Import key" : root.mode === "watch" ? "Watch" : "Create key"
    iconText: "󰌆"
    iconSpinning: root.busy
    bordered: true
    foreground: root.foreground
    onClicked: if (!root.busy) root.submit()
  }
}
