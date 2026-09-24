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

  function submit() {
    error = ""
    if (mode === "watch") {
      var npub = watchField.text.trim()
      if (npub.indexOf("npub1") !== 0) { error = "Paste an npub."; return }
      svc.call("config.set", { identity: { mode: "read-only", npub: npub }, modules: { notifications: true } }, function(err, r) {
        if (err) { root.error = err; return }
        root.svc.config = r
      })
      return
    }
    if (mode === "import" && secret.text.trim() === "") { error = "Paste your key first."; return }
    if (pass1.text.length < 8) { error = "Use a passphrase of at least 8 characters."; return }
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
      { value: "watch", label: "Just watch an npub" }
    ]
    value: root.mode
    foreground: root.foreground
    onChanged: function(v) { root.mode = v }
  }

  Text {
    width: parent.width
    visible: root.mode === "watch"
    wrapMode: Text.Wrap
    color: root.dim
    font.family: Style.font.family
    font.pixelSize: Style.font.bodySmall
    text: "Read-only: see notifications for any npub without a key. You can add your key later to sign and read DMs."
  }
  TextField {
    id: watchField
    width: parent.width
    visible: root.mode === "watch"
    placeholderText: "npub1…"
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
    placeholderText: "Passphrase (8+ characters)"
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
    width: parent.width
    visible: root.error !== ""
    wrapMode: Text.Wrap
    color: Color.urgent
    font.family: Style.font.family
    font.pixelSize: Style.font.bodySmall
    text: root.error
  }

  Button {
    text: root.busy ? "Encrypting…" : root.mode === "import" ? "Import key" : root.mode === "watch" ? "Watch" : "Create key"
    iconText: "󰌆"
    iconSpinning: root.busy
    bordered: true
    foreground: root.foreground
    onClicked: if (!root.busy) root.submit()
  }
}
