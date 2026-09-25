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
  onVisibleChanged: if (!visible) {
    oldPass.text = ""; newPass.text = ""; newPass2.text = ""; changingPass = false; passError = ""
    if (bunkerField) bunkerField.text = ""
  }
  property string passError: ""

  spacing: Style.space(10)

  function set(patch) {
    svc.runGuarded("config.set", patch, function(r) { root.svc.config = r },
      "This makes Opal lock less often. Confirm with your Opal passphrase.")
  }

  function changePassphrase() {
    passError = ""
    if (newPass.text.length < 10) { passError = "Use at least 10 characters (a few random words work well)."; return }
    if (newPass.text !== newPass2.text) { passError = "The new passphrases don't match."; return }
    svc.call("passphrase.change", { old: oldPass.text, new: newPass.text }, function(err) {
      if (err) { root.passError = err; oldPass.text = ""; return }
      oldPass.text = ""; newPass.text = ""; newPass2.text = ""
      root.changingPass = false
      root.svc.message("Passphrase changed", false)
    })
  }

  Text {
    textFormat: Text.PlainText
    width: parent.width
    visible: root.watching
    wrapMode: Text.Wrap
    color: root.dim
    font.family: Style.font.family
    font.pixelSize: Style.font.caption
    text: "You're watching someone (read-only). Manage it in Profiles."
  }
  PanelSectionHeader { text: "SIGN WITH"; foreground: root.dim }
  // Signer settings only mean something with a key stored in Opal.
  readonly property bool hasKey: !!root.svc && root.svc.hasAccounts === true
  // Settings that only concern answering apps over NIP-46.
  readonly property bool signerOn: hasKey && root.svc.signerOn === true
  readonly property bool watching: !!root.cfg.identity && root.cfg.identity.mode === "read-only"
  readonly property bool external: !!root.cfg.identity && root.cfg.identity.mode === "external"
  property string idMode: watching ? "read-only" : external ? "external" : "local"
  property bool bunkerBusy: false
  property string bunkerError: ""
  function connectBunker() {
    var uri = bunkerField.text.trim()
    if (uri.indexOf("bunker://") !== 0) { bunkerError = "Paste a bunker:// link from your signer app."; return }
    bunkerBusy = true
    bunkerError = ""
    svc.call("identity.external", { uri: uri }, function(err) {
      root.bunkerBusy = false
      if (err) { root.bunkerError = err; return }
      bunkerField.text = ""
      root.svc.refreshConfig()
      root.svc.message("Connected to your external signer", false)
    })
  }

  ButtonGroup {
    width: parent.width
    options: [
      { value: "local", label: "My key", tooltip: "Use the account selected in Profiles" },
      { value: "external", label: "External signer", tooltip: "Sign with a bunker such as Amber on your phone" }
    ]
    value: root.idMode
    foreground: root.foreground
    onChanged: function(v) {
      root.idMode = v
      if (v === "local") root.svc.run("identity.local", null, function() { root.svc.refreshConfig() })
    }
  }
  Text {
    textFormat: Text.PlainText
    width: parent.width
    visible: root.external
    elide: Text.ElideMiddle
    color: root.dim
    font.family: Style.font.family
    font.pixelSize: Style.font.caption
    text: root.external ? "Signing through your external signer as " + (root.cfg.identity.npub || "") : ""
  }
  Row {
    width: parent.width
    visible: root.idMode === "external"
    spacing: Style.space(8)
    TextField {
      id: bunkerField
      width: parent.width - bunkerButton.width - parent.spacing
      placeholderText: "bunker://… from Amber or another signer"
      password: true
      foreground: root.foreground
      onAccepted: root.connectBunker()
    }
    Button {
      id: bunkerButton
      anchors.verticalCenter: bunkerField.verticalCenter
      text: root.bunkerBusy ? "Approve on your signer…" : "Connect"
      iconSpinning: root.bunkerBusy
      bordered: true
      foreground: root.foreground
      onClicked: root.connectBunker()
    }
  }
  Text {
    textFormat: Text.PlainText
    width: parent.width
    visible: root.bunkerError !== ""
    wrapMode: Text.Wrap
    color: root.urgent
    font.family: Style.font.family
    font.pixelSize: Style.font.bodySmall
    text: root.bunkerError
  }
  PanelSectionHeader { text: "LOCK AFTER"; foreground: root.dim; visible: root.hasKey }
  ButtonGroup {
    width: parent.width
    visible: root.hasKey
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
    visible: root.hasKey
    label: "Lock with the screen"
    description: "Also locks before suspend"
    checked: root.signer.lock_on_screen_lock !== false
    foreground: root.foreground
    onClicked: root.set({ signer: { lock_on_screen_lock: !checked } })
  }

  PanelSectionHeader { text: "NEW APPS START WITH"; foreground: root.dim; visible: root.signerOn }
  ButtonGroup {
    width: parent.width
    visible: root.signerOn
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
    visible: root.signerOn
    label: "Privacy mode"
    description: "Don't keep an activity history (applies after a restart)"
    checked: root.signer.privacy_mode === true
    foreground: root.foreground
    onClicked: root.set({ signer: { privacy_mode: !checked } })
  }

  Toggle {
    width: parent.width
    visible: root.signerOn
    label: "Connected to relays"
    description: "Turn off to stop answering every app at once"
    checked: root.svc ? root.svc.online : true
    foreground: root.foreground
    onClicked: root.svc.run("online.set", { online: !checked })
  }

  PanelSectionHeader { text: "MODULES"; foreground: root.dim }
  Toggle {
    width: parent.width
    visible: root.hasKey
    label: "Log in to apps (signer)"
    description: "Let Nostr apps use your key through bunker links (NIP-46). Off: no app can use it; statuses and DMs still work."
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
          { key: "dms", label: "DMs", needsKey: true }
        ].filter(function(o) { return !o.needsKey || (!!root.svc && root.svc.canReadDms) })
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
      visible: !!root.svc && root.svc.canReadDms
      label: "Show DM text in popups"
      description: "Off keeps message text out of notifications and the database"
      checked: parent.n.dm_previews === true
      foreground: root.foreground
      onClicked: root.set({ notifications: { dm_previews: !checked } })
    }
  }

  Toggle {
    width: parent.width
    visible: !!root.svc && root.svc.canSign
    label: "Status"
    description: "Now playing, your status, and scrobbles (NIP-38)"
    checked: root.modules.status === true
    foreground: root.foreground
    onClicked: root.set({ modules: { status: !checked } })
  }

  Column {
    width: parent.width
    visible: root.modules.status === true && !!root.svc && root.svc.canSign
    spacing: Style.space(8)
    leftPadding: Style.space(12)

    readonly property var st: root.cfg.status || ({})
    readonly property real w: width - leftPadding

    Toggle {
      width: parent.w
      label: "Share what I'm listening to"
      description: "Any MPRIS player: Spotify, browsers, mpv…"
      checked: parent.st.music !== false
      foreground: root.foreground
      onClicked: root.set({ status: { music: !checked } })
    }
    PanelSectionHeader { text: "SONG LINK"; foreground: root.dim }
    ButtonGroup {
      width: parent.w
      options: [
        { value: "auto", label: "Auto", tooltip: "The player's own link (e.g. Spotify), else a search" },
        { value: "youtube-music", label: "YT Music" },
        { value: "spotify", label: "Spotify" },
        { value: "none", label: "None" }
      ]
      value: parent.st.music_link || "auto"
      foreground: root.foreground
      onChanged: function(v) { root.set({ status: { music_link: v } }) }
    }
    TextField {
      width: parent.w
      placeholderText: "Ignore players (comma separated, e.g. chromium, mpv)"
      text: (parent.st.players_blocked || []).join(", ")
      foreground: root.foreground
      onAccepted: root.set({ status: { players_blocked: text.split(",").map(function(x) { return x.trim() }).filter(function(x) { return x !== "" }) } })
    }
    Toggle {
      width: parent.w
      label: "Keep a listening history"
      description: "Stored on this computer (scrobbles)"
      checked: parent.st.scrobble !== false
      foreground: root.foreground
      onClicked: root.set({ status: { scrobble: !checked } })
    }
    Toggle {
      width: parent.w
      visible: parent.st.scrobble !== false
      label: "Publish scrobbles"
      description: "Each play as a public kind 1073 event (draft NIP)"
      checked: parent.st.publish_scrobbles === true
      foreground: root.foreground
      onClicked: root.set({ status: { publish_scrobbles: !checked } })
    }
    PanelSectionHeader { text: "AUTOMATIC STATUS"; foreground: root.dim }
    Toggle {
      width: parent.w
      label: "Calendar"
      description: "\"" + (parent.st.calendar_text || "In a meeting") + "\" during khal events"
      checked: parent.st.auto_calendar === true
      foreground: root.foreground
      onClicked: root.set({ status: { auto_calendar: !checked } })
    }
    Toggle {
      width: parent.w
      visible: parent.st.auto_calendar === true
      label: "Show event titles"
      description: "Off: just \"" + (parent.st.calendar_text || "In a meeting") + "\""
      checked: parent.st.calendar_titles === true
      foreground: root.foreground
      onClicked: root.set({ status: { calendar_titles: !checked } })
    }
    Toggle {
      width: parent.w
      label: "Away when locked"
      description: "\"" + (parent.st.away_text || "Away") + "\" while the screen is locked"
      checked: parent.st.auto_away === true
      foreground: root.foreground
      onClicked: root.set({ status: { auto_away: !checked } })
    }
    Toggle {
      width: parent.w
      label: "Focus with Do Not Disturb"
      description: "\"" + (parent.st.focus_text || "Focusing") + "\" while notifications are silenced"
      checked: parent.st.auto_focus === true
      foreground: root.foreground
      onClicked: root.set({ status: { auto_focus: !checked } })
    }
    Row {
      width: parent.w
      spacing: Style.space(6)
      TextField {
        width: (parent.width - 2 * parent.spacing) / 3
        placeholderText: "Meeting text"
        text: parent.parent.st.calendar_text || ""
        foreground: root.foreground
        onAccepted: if (text.trim() !== "") root.set({ status: { calendar_text: text.trim() } })
      }
      TextField {
        width: (parent.width - 2 * parent.spacing) / 3
        placeholderText: "Away text"
        text: parent.parent.st.away_text || ""
        foreground: root.foreground
        onAccepted: if (text.trim() !== "") root.set({ status: { away_text: text.trim() } })
      }
      TextField {
        width: (parent.width - 2 * parent.spacing) / 3
        placeholderText: "Focus text"
        text: parent.parent.st.focus_text || ""
        foreground: root.foreground
        onAccepted: if (text.trim() !== "") root.set({ status: { focus_text: text.trim() } })
      }
    }
  }

  PanelSectionHeader { text: "PASSPHRASE"; foreground: root.dim; visible: root.hasKey }
  Button {
    visible: root.hasKey && !root.changingPass
    text: "Change passphrase"
    iconText: "󰌆"
    bordered: true
    foreground: root.foreground
    onClicked: root.changingPass = true
  }
  Column {
    width: parent.width
    visible: root.hasKey && root.changingPass
    spacing: Style.space(8)
    TextField { id: oldPass; width: parent.width; password: true; placeholderText: "Current passphrase"; foreground: root.foreground }
    TextField { id: newPass; width: parent.width; password: true; placeholderText: "New passphrase"; foreground: root.foreground }
    TextField { id: newPass2; width: parent.width; password: true; placeholderText: "Repeat new passphrase"; foreground: root.foreground; onAccepted: root.changePassphrase() }
    Text {
      textFormat: Text.PlainText
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
    textFormat: Text.PlainText
    width: parent.width
    wrapMode: Text.Wrap
    color: root.dim
    font.family: Style.font.family
    font.pixelSize: Style.font.caption
    visible: root.signerOn
    text: "Signer relays: " + (root.signer.relays || []).join(", ")
  }
}
