import QtQuick
import qs.Commons
import qs.Ui
import "util.js" as U

// Accounts: switch, add, back up, remove.
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

  readonly property var accounts: svc ? svc.accounts : []
  readonly property var watched: svc && svc.readOnly ? (svc.status.watched || {}) : null
  property bool confirmUnwatch: false

  function add() {
    error = ""
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

  // Someone being watched (read-only).
  PanelSectionHeader { visible: !!root.watched; text: "WATCHING"; foreground: root.dim }
  CursorSurface {
    width: root.width
    visible: !!root.watched
    foreground: root.foreground
    implicitHeight: watchRow.implicitHeight + Style.spacing.xl
    Row {
      id: watchRow
      anchors.left: parent.left
      anchors.right: parent.right
      anchors.leftMargin: Style.space(8)
      anchors.rightMargin: Style.space(8)
      anchors.verticalCenter: parent.verticalCenter
      spacing: Style.space(10)
      Avatar {
        anchors.verticalCenter: parent.verticalCenter
        size: Style.space(32)
        picture: root.watched && root.watched.picture ? root.watched.picture : ""
        foreground: root.foreground
      }
      Column {
        width: parent.width - Style.space(42) - unwatchButton.width - Style.space(10)
        anchors.verticalCenter: parent.verticalCenter
        spacing: Style.space(1)
        Text {
          width: parent.width
          elide: Text.ElideRight
          color: root.foreground
          font.family: Style.font.family
          font.pixelSize: Style.font.body
          text: root.watched ? (root.watched.name || "Someone") + "  (read-only)" : ""
        }
        Text {
          width: parent.width
          elide: Text.ElideMiddle
          color: root.dim
          font.family: Style.font.family
          font.pixelSize: Style.font.caption
          text: root.watched ? (root.watched.nip05 || U.shortKey(root.watched.npub)) : ""
        }
      }
      Button {
        id: unwatchButton
        anchors.verticalCenter: parent.verticalCenter
        text: root.confirmUnwatch ? "Click again" : "Stop watching"
        bordered: true
        foreground: root.urgent
        onClicked: {
          if (!root.confirmUnwatch) { root.confirmUnwatch = true; return }
          root.confirmUnwatch = false
          root.svc.run("identity.unwatch", null, function() {
            root.svc.refreshConfig()
            root.svc.message("Stopped watching", false)
          })
        }
      }
    }
  }
  PanelSectionHeader { visible: !!root.watched && root.accounts.length > 0; text: "YOUR KEYS"; foreground: root.dim }

  Repeater {
    model: root.accounts
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
          picture: modelData.picture || ""
          foreground: root.foreground
        }
        Column {
          width: parent.width - Style.space(42) - actions.width
          anchors.verticalCenter: parent.verticalCenter
          spacing: Style.space(1)
          Text {
            width: parent.width
            elide: Text.ElideRight
            color: root.foreground
            font.family: Style.font.family
            font.pixelSize: Style.font.body
            font.bold: modelData.current
            text: modelData.label + (modelData.current ? "  (in use)" : "")
          }
          Text {
            width: parent.width
            elide: Text.ElideMiddle
            color: root.dim
            font.family: Style.font.family
            font.pixelSize: Style.font.caption
            text: modelData.nip05 || U.shortKey(modelData.npub)
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
            iconText: "󰌆"
            tooltipText: "Copy encrypted backup (ncryptsec)"
            onClicked: root.svc.run("accounts.export", { pubkey: modelData.pubkey }, function(r) {
              root.svc.copy(r.ncryptsec, "Encrypted backup copied")
            })
          }
          PanelActionButton {
            iconText: "󰆴"
            tooltipText: root.confirmRemove === modelData.pubkey ? "Click again to delete this account" : "Delete account"
            hoverColor: root.urgent
            onClicked: {
              if (root.confirmRemove !== modelData.pubkey) { root.confirmRemove = modelData.pubkey; return }
              root.confirmRemove = ""
              root.svc.run("accounts.remove", { pubkey: modelData.pubkey })
            }
          }
        }
      }
      MouseArea {
        anchors.fill: parent
        anchors.rightMargin: actions.width + Style.space(8)
        cursorShape: Qt.PointingHandCursor
        onClicked: if (!modelData.current) root.svc.run("accounts.select", { pubkey: modelData.pubkey })
      }
    }
  }

  Text {
    width: parent.width
    visible: root.confirmRemove !== ""
    wrapMode: Text.Wrap
    color: root.urgent
    font.family: Style.font.family
    font.pixelSize: Style.font.caption
    text: "Deleting removes the key from this computer and disconnects its apps. Make sure you have a backup."
  }

  Button {
    visible: !root.adding
    text: "Add account"
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
        { value: "import", label: "Import" },
        { value: "generate", label: "Create new" }
      ]
      value: root.addMode
      foreground: root.foreground
      onChanged: function(v) { root.addMode = v }
    }
    TextField {
      id: secret
      width: parent.width
      visible: root.addMode === "import"
      password: true
      placeholderText: "nsec, ncryptsec or recovery phrase"
      foreground: root.foreground
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
      placeholderText: "Nickname (optional)"
      foreground: root.foreground
    }
    TextField {
      id: pass
      width: parent.width
      password: true
      placeholderText: root.accounts.length === 0 ? "Choose an Opal passphrase (8+ characters)" : "Your Opal passphrase"
      foreground: root.foreground
      onAccepted: root.add()
    }
    Text {
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
        text: root.busy ? "Encrypting…" : "Add"
        iconSpinning: root.busy
        iconText: "󰌆"
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
