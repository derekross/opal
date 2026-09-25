import QtQuick
import qs.Commons
import qs.Ui
import "util.js" as U

// Profiles: your keys and a read-only (notifications only) profile, in one
// list. Click one to use it; add, back up or delete from here.
Column {
  id: root
  property var svc: null
  property color foreground: Color.foreground
  property color urgent: Color.urgent
  readonly property color dim: Qt.darker(foreground, 1.55)

  property bool adding: false
  property string addMode: "import"
  property bool busy: false
  property string error: ""
  property string confirmRemove: ""

  spacing: Style.space(10)

  // Never carry a pasted key from one mode into another (an nsec pasted
  // under Import must not end up as a "read-only" npub), and don't keep
  // secrets in fields once the page is gone.
  function clearFields() {
    secret.text = ""; ncPass.text = ""; pass.text = ""; nick.text = ""; error = ""
  }
  onAddModeChanged: clearFields()
  onVisibleChanged: if (!visible) { clearFields(); adding = false; confirmRemove = "" }

  readonly property var accounts: svc ? svc.accounts : []
  readonly property bool readOnlyActive: !!svc && svc.readOnly

  // One list: the read-only profile (if any) and every key.
  readonly property var profiles: {
    var out = []
    var w = svc && svc.status ? svc.status.watched : null
    if (w && w.npub) {
      out.push({
        readOnly: true,
        id: "watched",
        pubkey: "",
        npub: w.npub,
        label: w.name || "Read-only profile",
        subtitle: w.nip05 || U.shortKey(w.npub),
        picture: w.picture || "",
        current: w.active === true
      })
    }
    for (var i = 0; i < accounts.length; i++) {
      var a = accounts[i]
      out.push({
        readOnly: false,
        id: a.pubkey,
        pubkey: a.pubkey,
        npub: a.npub,
        label: a.label,
        subtitle: a.nip05 || U.shortKey(a.npub),
        picture: a.picture || "",
        current: a.current && !root.readOnlyActive
      })
    }
    return out
  }

  function use(p) {
    if (p.current) return
    if (p.readOnly) svc.run("identity.use_watched", null, function() { root.svc.refreshConfig() })
    else svc.run("accounts.select", { pubkey: p.pubkey }, function() { root.svc.refreshConfig() })
  }

  function remove(p) {
    if (confirmRemove !== p.id) { confirmRemove = p.id; return }
    confirmRemove = ""
    if (p.readOnly) svc.run("identity.unwatch", null, function() { root.svc.refreshConfig() })
    else svc.runGuarded("accounts.remove", { pubkey: p.pubkey }, null,
      "Deleting removes this key from the computer. Confirm with your Opal passphrase.")
  }

  function add() {
    error = ""
    if (addMode === "read-only") {
      var who = secret.text.trim()
      if (who === "") { error = "Enter an npub or a NIP-05 address."; return }
      busy = true
      svc.call("identity.watch", { input: who }, function(err) {
        root.busy = false
        if (err) { root.error = err; return }
        secret.text = ""
        root.adding = false
        root.svc.refreshConfig()
        root.svc.message("Added a read-only profile", false)
      })
      return
    }
    if (addMode === "import" && secret.text.trim() === "") { error = "Paste the key first."; return }
    if (pass.text === "") { error = "Enter your Opal passphrase."; return }
    busy = true
    var p = { passphrase: pass.text, nickname: nick.text }
    if (addMode === "import") {
      p.secret = secret.text.trim()
      if (p.secret.indexOf("ncryptsec1") === 0) p.ncryptsec_password = ncPass.text
    }
    svc.call("accounts.add", p, function(err) {
      root.busy = false
      if (err) { root.error = err; return }
      secret.text = ""; ncPass.text = ""; pass.text = ""; nick.text = ""
      root.adding = false
      root.svc.message("Account added", false)
    })
  }

  Repeater {
    model: root.profiles
    delegate: CursorSurface {
      required property var modelData
      width: root.width
      foreground: root.foreground
      current: modelData.current
      implicitHeight: accRow.implicitHeight + Style.spacing.xl

      Row {
        id: accRow
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.leftMargin: Style.space(8)
        anchors.rightMargin: Style.space(8)
        anchors.verticalCenter: parent.verticalCenter
        spacing: Style.space(10)

        Avatar {
          anchors.verticalCenter: parent.verticalCenter
          size: Style.space(32)
          picture: modelData.picture
          foreground: root.foreground
        }
        Column {
          width: parent.width - Style.space(42) - actions.width
          anchors.verticalCenter: parent.verticalCenter
          spacing: Style.space(1)
          Text {
            textFormat: Text.PlainText
            width: parent.width
            elide: Text.ElideRight
            color: root.foreground
            font.family: Style.font.family
            font.pixelSize: Style.font.body
            font.bold: modelData.current
            text: modelData.label + (modelData.current ? "  (in use)" : "")
          }
          Text {
            textFormat: Text.PlainText
            width: parent.width
            elide: Text.ElideMiddle
            color: root.dim
            font.family: Style.font.family
            font.pixelSize: Style.font.caption
            text: (modelData.readOnly ? "Read-only · notifications only · " : "") + modelData.subtitle
          }
        }
        Row {
          id: actions
          anchors.verticalCenter: parent.verticalCenter
          spacing: Style.space(2)
          PanelActionButton {
            iconText: "󰆏"
            tooltipText: "Copy npub"
            onClicked: root.svc.copy(modelData.npub, "npub copied")
          }
          PanelActionButton {
            visible: !modelData.readOnly
            iconText: "󰌆"
            tooltipText: "Copy encrypted backup (ncryptsec)"
            onClicked: root.svc.run("accounts.export", { pubkey: modelData.pubkey }, function(r) {
              root.svc.copy(r.ncryptsec, "Encrypted backup copied")
            })
          }
          PanelActionButton {
            iconText: "󰆴"
            tooltipText: root.confirmRemove === modelData.id
              ? "Click again to remove this profile"
              : (modelData.readOnly ? "Remove read-only profile" : "Delete account")
            hoverColor: root.urgent
            onClicked: root.remove(modelData)
          }
        }
      }
      MouseArea {
        anchors.fill: parent
        anchors.rightMargin: actions.width + Style.space(8)
        cursorShape: Qt.PointingHandCursor
        onClicked: root.use(modelData)
      }
    }
  }

  Text {
    textFormat: Text.PlainText
    width: parent.width
    visible: root.confirmRemove !== ""
    wrapMode: Text.Wrap
    color: root.urgent
    font.family: Style.font.family
    font.pixelSize: Style.font.caption
    text: root.confirmRemove === "watched"
      ? "Click the trash again to remove this read-only profile."
      : "Deleting removes the key from this computer and disconnects its apps. Make sure you have a backup."
  }

  Button {
    visible: !root.adding
    text: "Add profile"
    iconText: "󰐕"
    bordered: true
    foreground: root.foreground
    onClicked: root.adding = true
  }

  Column {
    width: parent.width
    visible: root.adding
    spacing: Style.space(8)

    ButtonGroup {
      width: parent.width
      options: [
        { value: "import", label: "Import key" },
        { value: "generate", label: "New key" },
        { value: "read-only", label: "Read-only", tooltip: "Notifications only, for an npub or NIP-05 address" }
      ]
      value: root.addMode
      foreground: root.foreground
      onChanged: function(v) { root.addMode = v; root.error = "" }
    }
    Text {
      textFormat: Text.PlainText
      width: parent.width
      visible: root.addMode === "read-only"
      wrapMode: Text.Wrap
      color: root.dim
      font.family: Style.font.family
      font.pixelSize: Style.font.bodySmall
      text: "See notifications for anyone, no key needed. Enter an npub or a NIP-05 address like you@example.com."
    }
    TextField {
      id: secret
      width: parent.width
      visible: root.addMode !== "generate"
      password: root.addMode !== "read-only"
      placeholderText: root.addMode === "read-only" ? "npub1… or name@domain" : "nsec, ncryptsec or recovery phrase"
      foreground: root.foreground
      onAccepted: if (root.addMode === "read-only") root.add()
    }
    TextField {
      id: ncPass
      width: parent.width
      visible: root.addMode === "import" && secret.text.trim().indexOf("ncryptsec1") === 0
      password: true
      placeholderText: "Password of that ncryptsec"
      foreground: root.foreground
    }
    TextField {
      id: nick
      width: parent.width
      visible: root.addMode !== "read-only"
      placeholderText: "Nickname (optional)"
      foreground: root.foreground
    }
    TextField {
      id: pass
      width: parent.width
      visible: root.addMode !== "read-only"
      password: true
      placeholderText: root.accounts.length === 0 ? "Choose an Opal passphrase (a few random words)" : "Your Opal passphrase"
      foreground: root.foreground
      onAccepted: root.add()
    }
    Text {
      textFormat: Text.PlainText
      width: parent.width
      visible: root.error !== ""
      wrapMode: Text.Wrap
      color: root.urgent
      font.family: Style.font.family
      font.pixelSize: Style.font.bodySmall
      text: root.error
    }
    Row {
      spacing: Style.space(8)
      Button {
        text: root.busy ? (root.addMode === "read-only" ? "Looking up…" : "Encrypting…") : "Add"
        iconSpinning: root.busy
        iconText: "󰐕"
        bordered: true
        foreground: root.foreground
        onClicked: if (!root.busy) root.add()
      }
      Button {
        text: "Cancel"
        foreground: root.foreground
        onClicked: { root.adding = false; root.error = "" }
      }
    }
  }
}
