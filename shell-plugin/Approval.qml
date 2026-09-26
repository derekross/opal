import QtQuick
import QtQuick.Controls
import Quickshell
import Quickshell.Wayland
import qs.Commons
import qs.Ui
import "util.js" as U

// Modal shown when something needs a decision: a new app wants to connect,
// an app wants a signature, or an app is waiting for an unlock.
Item {
  id: root

  // Injected by omarchy-shell.
  property var shell: null
  property var manifest: null
  property var service: null

  property bool opened: false

  readonly property var svc: service || (shell && typeof shell.serviceFor === "function" ? shell.serviceFor((manifest && manifest.id) || "derekross.opal") : null)
  readonly property color foreground: Color.popups.text
  readonly property color dim: Qt.darker(foreground, 1.55)
  readonly property color urgent: Color.urgent

  readonly property var offer: svc && svc.signerOn && svc.offers.length > 0 ? svc.offers[0] : null
  // A program on this computer asking to pair (as opposed to a nostrconnect:// link).
  readonly property bool localOffer: !!offer && offer.type === "local"
  readonly property var prompt: svc && svc.signerOn && svc.prompts.length > 0 ? svc.prompts[0] : null
  readonly property bool needUnlock: !!svc && svc.locked && svc.hasAccounts && (!!svc.unlockRequest || !!prompt)
  readonly property string mode: !svc ? "none"
    : needUnlock ? "unlock"
    : offer ? "offer"
    : prompt ? "prompt"
    : "none"

  property string remember: "once"
  property bool showRaw: false
  property string policy: "basic"
  property var granted: ({})
  property bool busy: false
  property string error: ""

  function open(payloadJson) {
    opened = true
    reset()
    armed = mode === "unlock"
    armTimer.restart()
    Qt.callLater(focusDefault)
  }
  function close() { opened = false }
  function dismiss() {
    opened = false
    if (shell && typeof shell.hide === "function") shell.hide((manifest && manifest.id) || "derekross.opal")
  }
  function focusDefault() {
    if (mode === "unlock") passField.forceActiveFocus()
    else keyCatcher.forceActiveFocus()
  }
  function reset() {
    remember = "once"
    showRaw = false
    error = ""
    busy = false
    policy = svc && svc.config.signer ? (svc.config.signer.default_policy || "basic") : "basic"
    if (policy === "full-trust") policy = "basic"
    // Nothing is granted unless you tick it.
    granted = ({})
  }

  // What's on screen. Whenever it changes (a new request, another app's
  // request taking its place), every choice resets and the buttons stay
  // disabled for a moment, so a click meant for one thing can't land on
  // another.
  readonly property string shownId: mode === "offer" ? "o:" + offer.id
    : mode === "prompt" ? "p:" + prompt.id
    : mode
  property bool armed: false
  onShownIdChanged: {
    if (mode === "none") {
      if (opened) dismiss()
      return
    }
    reset()
    armed = mode === "unlock"
    armTimer.restart()
    Qt.callLater(focusDefault)
  }
  Timer {
    id: armTimer
    interval: 900
    onTriggered: root.armed = true
  }

  function answer(allow) {
    if (!prompt || busy || (allow && !armed)) return
    busy = true
    svc.call("prompts.answer", { id: prompt.id, allow: allow, remember: remember }, function(err) {
      root.busy = false
      if (err) root.error = err
      root.svc.refreshPrompts()
    })
  }
  function acceptOffer() {
    if (!offer || busy || !armed) return
    busy = true
    var grant = []
    for (var k in granted) if (granted[k]) grant.push(k)
    var local = localOffer
    svc.call(local ? "app.accept" : "nostrconnect.accept", { offer_id: offer.id, policy: policy, grant: grant }, function(err, r) {
      root.busy = false
      if (err) { root.error = err; return }
      root.svc.message((local ? "Paired " : "Connected ") + (r.display_name || "app"), false)
      root.svc.refreshOffers()
      root.svc.refreshApps()
    })
  }
  function rejectOffer() {
    if (!offer) return
    svc.call(localOffer ? "app.reject" : "nostrconnect.reject", { id: offer.id }, function() { root.svc.refreshOffers() })
  }
  function unlock() {
    if (busy || passField.text === "") return
    busy = true
    error = ""
    svc.call("unlock", { passphrase: passField.text }, function(err) {
      root.busy = false
      passField.text = ""
      if (err) { root.error = err; Qt.callLater(root.focusDefault) }
    })
  }

  readonly property string eventPreview: {
    if (!prompt || !prompt.event) return ""
    var c = prompt.event.content || ""
    return c.length > 600 ? c.slice(0, 600) + "…" : c
  }
  readonly property string rawJson: prompt && prompt.event ? JSON.stringify(prompt.event, null, 2) : ""

  readonly property string keyTags: {
    if (!prompt || !prompt.event) return ""
    var want = ["u", "method", "t", "p", "e", "relay", "expiration", "challenge"]
    var lines = []
    var tags = prompt.event.tags || []
    for (var i = 0; i < tags.length && lines.length < 8; i++) {
      var t = tags[i]
      if (t.length > 1 && want.indexOf(t[0]) !== -1) {
        var v = String(t[1])
        lines.push(t[0] + ": " + (v.length > 80 ? v.slice(0, 80) + "…" : v))
      }
    }
    return lines.join("\n")
  }
  readonly property string dateWarning: {
    if (!prompt || !prompt.event) return ""
    var d = prompt.event.created_at - Math.floor(Date.now() / 1000)
    if (Math.abs(d) < 600) return ""
    var h = Math.round(Math.abs(d) / 360) / 10
    return "Dated " + h + " hours " + (d > 0 ? "in the future" : "in the past") + " (" + U.clock(prompt.event.created_at) + ")."
  }

  PanelWindow {
    id: window
    visible: root.opened
    anchors { top: true; bottom: true; left: true; right: true }
    color: "transparent"
    WlrLayershell.namespace: "opal-approval"
    WlrLayershell.layer: WlrLayer.Overlay
    WlrLayershell.keyboardFocus: WlrKeyboardFocus.Exclusive
    exclusionMode: ExclusionMode.Ignore

    Rectangle {
      anchors.fill: parent
      color: Color.menu.scrim
    }
    MouseArea {
      anchors.fill: parent
      onClicked: root.focusDefault()
    }

    BorderSurface {
      id: card
      anchors.centerIn: parent
      width: Math.min(Style.space(460), window.width - Style.space(40))
      implicitHeight: content.implicitHeight + Style.space(40)
      radius: Style.cornerRadius
      color: Color.popups.background
      borderSpec: Border.surfaceSpec("popups", "border", Color.popups.border, Math.max(1, Style.space(2)))

      MouseArea { anchors.fill: parent; onClicked: {} }

      Item {
        id: keyCatcher
        anchors.fill: parent
        focus: true
        Keys.priority: Keys.BeforeItem
        Keys.onPressed: function(event) {
          if (event.key === Qt.Key_Escape) {
            root.dismiss()
            event.accepted = true
          }
        }

        Column {
          id: content
          anchors.left: parent.left
          anchors.right: parent.right
          anchors.top: parent.top
          anchors.margins: Style.space(20)
          spacing: Style.space(12)

          // Title
          Row {
            width: parent.width
            spacing: Style.space(10)
            Avatar {
              anchors.verticalCenter: parent.verticalCenter
              size: Style.space(36)
              foreground: root.foreground
              // App images are the app's own claim; don't fetch them here.
              picture: ""
            }
            Column {
              width: parent.width - Style.space(46)
              anchors.verticalCenter: parent.verticalCenter
              spacing: Style.space(2)
              Text {
                textFormat: Text.PlainText
                width: parent.width
                wrapMode: Text.Wrap
                color: root.foreground
                font.family: Style.font.family
                font.pixelSize: Style.font.heading
                font.bold: true
                text: {
                  switch (root.mode) {
                  case "unlock": return "Unlock Opal"
                  case "offer": return (root.offer.name || "An app") + (root.localOffer ? " wants to use your key" : " wants to connect")
                  case "prompt": return root.prompt.app_name
                  }
                  return ""
                }
              }
              Text {
                textFormat: Text.PlainText
                width: parent.width
                wrapMode: Text.Wrap
                color: root.dim
                font.family: Style.font.family
                font.pixelSize: Style.font.caption
                text: {
                  switch (root.mode) {
                  case "unlock":
                    return root.svc.unlockRequest ? root.svc.unlockRequest.app_name + " is waiting for your signature" : "Requests are waiting"
                  case "offer":
                    if (root.localOffer)
                      return "A program on this computer · " + (root.offer.exe || root.offer.unit || ("unknown program (pid " + root.offer.pid + ")"))
                    return (root.offer.url ? root.offer.url + " (as the app claims) · " : "Unverified app · ")
                      + root.offer.relays.join(", ")
                  case "prompt":
                    var n = root.svc.prompts.length
                    return (root.prompt.app_url || "") + (n > 1 ? "  ·  1 of " + n : "")
                  }
                  return ""
                }
              }
            }
          }

          // ── Unlock ─────────────────────────────────────────────
          TextField {
            id: passField
            width: parent.width
            visible: root.mode === "unlock"
            password: true
            placeholderText: "Opal passphrase"
            foreground: root.foreground
            enabled: !root.busy
            onAccepted: root.unlock()
            Keys.onEscapePressed: root.dismiss()
          }

          // ── Connect offer ──────────────────────────────────────
          Column {
            width: parent.width
            visible: root.mode === "offer"
            spacing: Style.space(8)

            Text {
              textFormat: Text.PlainText
              width: parent.width
              wrapMode: Text.Wrap
              color: root.foreground
              font.family: Style.font.family
              font.pixelSize: Style.font.body
              text: "It will sign as " + (root.localOffer && root.offer.account_label ? root.offer.account_label
                : root.svc && root.svc.currentAccount ? root.svc.currentAccount.label : "your account") + "."
            }

            // What a local program declared. Sensitive kinds (relay logins,
            // Blossom uploads…) can't be pre-allowed: they always ask.
            PanelSectionHeader {
              visible: root.localOffer
              text: "WHAT IT MAY ASK FOR"
              foreground: root.dim
            }
            Repeater {
              model: root.localOffer ? root.offer.kinds : []
              delegate: Text {
                required property var modelData
                textFormat: Text.PlainText
                width: parent ? parent.width : 0
                wrapMode: Text.Wrap
                color: root.foreground
                font.family: Style.font.family
                font.pixelSize: Style.font.bodySmall
                text: "Sign: " + modelData.label + " (kind " + modelData.kind + ")" + (modelData.sensitive ? " · always asks" : "")
              }
            }
            Text {
              textFormat: Text.PlainText
              visible: root.localOffer && root.offer.nip44
              width: parent.width
              wrapMode: Text.Wrap
              color: root.foreground
              font.family: Style.font.family
              font.pixelSize: Style.font.bodySmall
              text: "Encrypt and decrypt its own data with your key (NIP-44) · decrypting asks"
            }
            Text {
              textFormat: Text.PlainText
              visible: root.localOffer
              width: parent.width
              wrapMode: Text.Wrap
              color: root.dim
              font.family: Style.font.family
              font.pixelSize: Style.font.caption
              text: "Only this program, started the same way, can use the pairing. You can change or revoke it under Apps."
            }
            PanelSectionHeader { text: "WHEN IT ASKS"; foreground: root.dim }
            // Full trust needs the passphrase, which this dialog doesn't
            // take; it can be given later under Apps.
            ButtonGroup {
              width: parent.width
              options: U.policyOptions.filter(function(o) { return o.value !== "full-trust" })
              value: root.policy
              foreground: root.foreground
              onChanged: function(v) { root.policy = v }
            }
            Text {
              textFormat: Text.PlainText
              width: parent.width
              wrapMode: Text.Wrap
              color: root.dim
              font.family: Style.font.family
              font.pixelSize: Style.font.caption
              text: "Full trust can be given later under Apps, with your passphrase."
            }
            PanelSectionHeader {
              visible: root.offer && root.offer.perms.length > 0
              text: "IT ASKS TO ALWAYS ALLOW (TICK WHAT YOU AGREE TO)"
              foreground: root.dim
            }
            Repeater {
              model: root.offer ? root.offer.perms : []
              delegate: Toggle {
                required property var modelData
                width: parent ? parent.width : 0
                label: U.describe(modelData.method, modelData.label)
                description: modelData.perm
                checked: root.granted[modelData.perm] === true
                foreground: root.foreground
                onClicked: {
                  var g = Object.assign({}, root.granted)
                  g[modelData.perm] = !checked
                  root.granted = g
                }
              }
            }
          }

          // ── Signing / encryption prompt ────────────────────────
          Column {
            width: parent.width
            visible: root.mode === "prompt"
            spacing: Style.space(8)

            Text {
              textFormat: Text.PlainText
              width: parent.width
              wrapMode: Text.Wrap
              color: root.foreground
              font.family: Style.font.family
              font.pixelSize: Style.font.title
              text: root.prompt ? U.describe(root.prompt.method, root.prompt.kind_label) : ""
            }
            Text {
              textFormat: Text.PlainText
              width: parent.width
              visible: !!root.prompt && !!root.prompt.counterparty
              wrapMode: Text.Wrap
              color: root.dim
              font.family: Style.font.family
              font.pixelSize: Style.font.caption
              text: root.prompt && root.prompt.counterparty
                ? "With " + U.shortKey(root.prompt.counterparty) + " · " + root.prompt.payload_len + " characters"
                : ""
            }

            BorderSurface {
              width: parent.width
              visible: root.eventPreview !== "" || root.showRaw
              color: "transparent"
              borderSpec: Border.controlSpec("normal", root.foreground, Color.accent)
              implicitHeight: Math.min(previewText.implicitHeight + Style.space(16), Style.space(220))
              clip: true
              Flickable {
                anchors.fill: parent
                anchors.margins: Style.space(8)
                contentHeight: previewText.implicitHeight
                clip: true
                boundsBehavior: Flickable.StopAtBounds
                Text {
                  textFormat: Text.PlainText
                  id: previewText
                  width: parent.width
                  wrapMode: Text.WrapAnywhere
                  color: root.foreground
                  font.family: Style.font.family
                  font.pixelSize: Style.font.bodySmall
                  text: root.showRaw ? root.rawJson : root.eventPreview
                }
              }
            }

            Row {
              spacing: Style.space(8)
              visible: !!root.prompt && !!root.prompt.event
              Text {
                textFormat: Text.PlainText
                anchors.verticalCenter: parent.verticalCenter
                color: root.dim
                font.family: Style.font.family
                font.pixelSize: Style.font.caption
                text: root.prompt && root.prompt.event ? root.prompt.event.tags.length + " tags · kind " + root.prompt.kind : ""
              }
              Button {
                text: root.showRaw ? "Hide JSON" : "Show JSON"
                foreground: root.dim
                onClicked: root.showRaw = !root.showRaw
              }
            }

            // Tags that decide what this does (where an auth token is for,
            // who a note tags…), and a warning for odd dates.
            Text {
              textFormat: Text.PlainText
              width: parent.width
              visible: text !== ""
              wrapMode: Text.WrapAnywhere
              color: root.foreground
              font.family: Style.font.family
              font.pixelSize: Style.font.caption
              text: root.keyTags
            }
            Text {
              textFormat: Text.PlainText
              width: parent.width
              visible: text !== ""
              wrapMode: Text.Wrap
              color: root.urgent
              font.family: Style.font.family
              font.pixelSize: Style.font.bodySmall
              text: root.dateWarning
            }

            PanelSectionHeader { text: "REMEMBER THIS ANSWER"; foreground: root.dim }
            Text {
              textFormat: Text.PlainText
              width: parent.width
              visible: !!root.prompt && root.remember !== "once"
                && (root.prompt.method === "nip04_decrypt" || root.prompt.method === "nip44_decrypt"
                    || root.prompt.method === "nip04_encrypt" || root.prompt.method === "nip44_encrypt")
              wrapMode: Text.Wrap
              color: root.urgent
              font.family: Style.font.family
              font.pixelSize: Style.font.caption
              text: "A remembered answer covers every conversation, not just this one."
            }
            ButtonGroup {
              width: parent.width
              // Sensitive requests can't be remembered for long.
              options: root.prompt && root.prompt.sensitive
                ? U.rememberOptions.filter(function(o) { return ["once", "5m", "1h"].indexOf(o.value) !== -1 })
                : U.rememberOptions
              value: root.remember
              foreground: root.foreground
              onChanged: function(v) { root.remember = v }
            }
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

          // ── Actions ────────────────────────────────────────────
          Row {
            anchors.right: parent.right
            spacing: Style.space(8)

            Button {
              text: "Later"
              foreground: root.dim
              tooltipText: "Close this; requests keep waiting until they time out"
              onClicked: root.dismiss()
            }
            Button {
              visible: root.mode === "offer" || root.mode === "prompt"
              text: root.mode === "offer" ? "Reject" : "Deny"
              iconText: "󰅖"
              bordered: true
              foreground: root.urgent
              onClicked: root.mode === "offer" ? root.rejectOffer() : root.answer(false)
            }
            Button {
              text: root.mode === "unlock" ? "Unlock" : root.mode === "offer" ? (root.localOffer ? "Pair" : "Connect") : "Approve"
              iconText: root.mode === "unlock" ? "󰿆" : "󰄬"
              iconSpinning: root.busy
              bordered: true
              active: root.armed
              enabled: root.armed
              foreground: root.foreground
              onClicked: {
                if (root.mode === "unlock") root.unlock()
                else if (root.mode === "offer") root.acceptOffer()
                else root.answer(true)
              }
            }
          }
        }
      }
    }
  }
}
