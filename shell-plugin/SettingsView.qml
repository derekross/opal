import QtQuick
import qs.Commons
import qs.Ui

// Signer settings, modules and the kill switch.
Column {
  id: root
  property var svc: null
  property color foreground: Color.foreground
  property color urgent: Color.urgent
  readonly property color dim: Qt.darker(foreground, 1.55)

  readonly property var cfg: svc ? svc.config : ({})
  readonly property var signer: cfg.signer || ({})
  readonly property var modules: cfg.modules || ({})

  property bool changingPass: false
  property string passError: ""

  spacing: Style.space(10)

  function set(patch) {
    svc.run("config.set", patch, function(r) { root.svc.config = r })
  }

  function changePassphrase() {
    passError = ""
    if (newPass.text.length < 8) { passError = "Use at least 8 characters."; return }
    if (newPass.text !== newPass2.text) { passError = "The new passphrases don't match."; return }
    svc.call("passphrase.change", { old: oldPass.text, new: newPass.text }, function(err) {
      if (err) { root.passError = err; return }
      oldPass.text = ""; newPass.text = ""; newPass2.text = ""
      root.changingPass = false
      root.svc.message("Passphrase changed", false)
    })
  }

  PanelSectionHeader { text: "IDENTITY"; foreground: root.dim }
  ButtonGroup {
    width: parent.width
    options: [
      { value: "local", label: "My key (signer)", tooltip: "Use the account selected in Profiles" },
      { value: "read-only", label: "Watch an npub", tooltip: "Notifications only, no key needed" }
    ]
    value: (root.cfg.identity && root.cfg.identity.mode === "read-only") ? "read-only" : "local"
    foreground: root.foreground
    onChanged: function(v) {
      if (v === "local") root.set({ identity: { mode: "local" } })
      else if (npubField.text.trim().indexOf("npub1") === 0) root.set({ identity: { mode: "read-only", npub: npubField.text.trim() } })
      else npubField.forceActiveFocus()
    }
  }
  TextField {
    id: npubField
    width: parent.width
    placeholderText: "npub to watch"
    text: root.cfg.identity && root.cfg.identity.npub ? root.cfg.identity.npub : ""
    foreground: root.foreground
    onAccepted: if (text.trim().indexOf("npub1") === 0) root.set({ identity: { mode: "read-only", npub: text.trim() } })
  }

  PanelSectionHeader { text: "LOCK AFTER"; foreground: root.dim }
  ButtonGroup {
    width: parent.width
    options: [
      { value: "5", label: "5m" },
      { value: "15", label: "15m" },
      { value: "60", label: "1h" },
      { value: "240", label: "4h" },
      { value: "1440", label: "1d" },
      { value: "never", label: "Never" }
    ]
    value: root.signer.auto_lock_minutes ? String(root.signer.auto_lock_minutes) : "never"
    foreground: root.foreground
    onChanged: function(v) {
      root.set({ signer: { auto_lock_minutes: v === "never" ? null : parseInt(v) } })
    }
  }

  Toggle {
    width: parent.width
    label: "Lock with the screen"
    description: "Also locks before suspend"
    checked: root.signer.lock_on_screen_lock !== false
    foreground: root.foreground
    onClicked: root.set({ signer: { lock_on_screen_lock: !checked } })
  }

  PanelSectionHeader { text: "NEW APPS START WITH"; foreground: root.dim }
  ButtonGroup {
    width: parent.width
    options: [
      { value: "basic", label: "Basic" },
      { value: "manual", label: "Ask for everything" }
    ]
    value: root.signer.default_policy || "basic"
    foreground: root.foreground
    onChanged: function(v) { root.set({ signer: { default_policy: v } }) }
  }

  Toggle {
    width: parent.width
    label: "Privacy mode"
    description: "Don't keep an activity history (applies after a restart)"
    checked: root.signer.privacy_mode === true
    foreground: root.foreground
    onClicked: root.set({ signer: { privacy_mode: !checked } })
  }

  Toggle {
    width: parent.width
    label: "Connected to relays"
    description: "Turn off to stop answering every app at once"
    checked: root.svc ? root.svc.online : true
    foreground: root.foreground
    onClicked: root.svc.run("online.set", { online: !checked })
  }

  PanelSectionHeader { text: "MODULES"; foreground: root.dim }
  Toggle {
    width: parent.width
    label: "Signer"
    description: "NIP-46 remote signing for your apps"
    checked: root.modules.signer !== false
    foreground: root.foreground
    onClicked: root.set({ modules: { signer: !checked } })
  }
  Toggle {
    width: parent.width
    label: "Notifications"
    description: "Replies, mentions, reactions, zaps and DMs"
    checked: root.modules.notifications === true
    foreground: root.foreground
    onClicked: root.set({ modules: { notifications: !checked } })
  }
  Column {
    width: parent.width
    visible: root.modules.notifications === true
    spacing: Style.space(8)
    leftPadding: Style.space(12)

    readonly property var n: root.cfg.notifications || ({})
    readonly property var types: n.types || ({})

    PanelSectionHeader { text: "SHOW"; foreground: root.dim }
    Flow {
      width: parent.width - parent.leftPadding
      spacing: Style.space(6)
      Repeater {
        model: [
          { key: "replies", label: "Replies" },
          { key: "mentions", label: "Mentions" },
          { key: "reposts", label: "Reposts" },
          { key: "reactions", label: "Reactions" },
          { key: "zaps", label: "Zaps" },
          { key: "dms", label: "DMs" }
        ]
        delegate: Button {
          required property var modelData
          text: modelData.label
          selected: parent.parent.types[modelData.key] !== false
          bordered: true
          foreground: root.foreground
          onClicked: {
            var t = {}
            t[modelData.key] = !selected
            root.set({ notifications: { types: t } })
          }
        }
      }
    }
    PanelSectionHeader { text: "OPEN IN"; foreground: root.dim }
    ButtonGroup {
      width: parent.width - parent.leftPadding
      options: root.svc && root.svc.notifyStatus.clients ? root.svc.notifyStatus.clients : []
      value: parent.n.client || "primal"
      foreground: root.foreground
      onChanged: function(v) { root.set({ notifications: { client: v } }) }
    }
    Toggle {
      width: parent.width - parent.leftPadding
      label: "Desktop popups"
      checked: parent.n.desktop !== false
      foreground: root.foreground
      onClicked: root.set({ notifications: { desktop: !checked } })
    }
    Toggle {
      width: parent.width - parent.leftPadding
      label: "Show DM text in popups"
      description: "Off keeps message text out of notifications and the database"
      checked: parent.n.dm_previews === true
      foreground: root.foreground
      onClicked: root.set({ notifications: { dm_previews: !checked } })
    }
  }

  Toggle {
    width: parent.width
    label: "Status"
    description: "Now playing and status updates (coming soon)"
    checked: root.modules.status === true
    foreground: root.foreground
    onClicked: root.set({ modules: { status: !checked } })
  }

  PanelSectionHeader { text: "PASSPHRASE"; foreground: root.dim }
  Button {
    visible: !root.changingPass
    text: "Change passphrase"
    iconText: "󰌆"
    bordered: true
    foreground: root.foreground
    onClicked: root.changingPass = true
  }
  Column {
    width: parent.width
    visible: root.changingPass
    spacing: Style.space(8)
    TextField { id: oldPass; width: parent.width; password: true; placeholderText: "Current passphrase"; foreground: root.foreground }
    TextField { id: newPass; width: parent.width; password: true; placeholderText: "New passphrase"; foreground: root.foreground }
    TextField { id: newPass2; width: parent.width; password: true; placeholderText: "Repeat new passphrase"; foreground: root.foreground; onAccepted: root.changePassphrase() }
    Text {
      width: parent.width
      visible: root.passError !== ""
      wrapMode: Text.Wrap
      color: root.urgent
      font.family: Style.font.family
      font.pixelSize: Style.font.bodySmall
      text: root.passError
    }
    Row {
      spacing: Style.space(8)
      Button { text: "Change"; bordered: true; foreground: root.foreground; onClicked: root.changePassphrase() }
      Button { text: "Cancel"; foreground: root.foreground; onClicked: { root.changingPass = false; root.passError = "" } }
    }
  }

  Text {
    width: parent.width
    wrapMode: Text.Wrap
    color: root.dim
    font.family: Style.font.family
    font.pixelSize: Style.font.caption
    text: "Signer relays: " + (root.signer.relays || []).join(", ")
  }
}
