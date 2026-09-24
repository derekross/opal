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

  readonly property var svc: service || (shell && typeof shell.serviceFor === "function" ? shell.serviceFor("opal") : null)
  readonly property color foreground: Color.popups.text
  readonly property color dim: Qt.darker(foreground, 1.55)
  readonly property color urgent: Color.urgent

  readonly property var offer: svc && svc.offers.length > 0 ? svc.offers[0] : null
  readonly property var prompt: svc && svc.prompts.length > 0 ? svc.prompts[0] : null
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
    Qt.callLater(focusDefault)
  }
  function close() { opened = false }
  function dismiss() {
    opened = false
    if (shell && typeof shell.hide === "function") shell.hide((manifest && manifest.id) || "opal")
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
    var g = {}
    if (offer) for (var i = 0; i < offer.perms.length; i++) g[offer.perms[i].perm] = true
    granted = g
  }

  // Close once nothing is left to decide.
  onModeChanged: {
    if (mode === "none" && opened) dismiss()
    else { reset(); Qt.callLater(focusDefault) }
  }

  function answer(allow) {
    if (!prompt || busy) return
    busy = true
    svc.call("prompts.answer", { id: prompt.id, allow: allow, remember: remember }, function(err) {
      root.busy = false
      if (err) root.error = err
      root.svc.refreshPrompts()
    })
  }
  function acceptOffer() {
    if (!offer || busy) return
    busy = true
    var grant = []
    for (var k in granted) if (granted[k]) grant.push(k)
    svc.call("nostrconnect.accept", { offer_id: offer.id, policy: policy, grant: grant }, function(err, r) {
      root.busy = false
      if (err) { root.error = err; return }
      root.svc.message("Connected " + (r.display_name || "app"), false)
      root.svc.refreshOffers()
      root.svc.refreshApps()
    })
  }
  function rejectOffer() {
    if (!offer) return
    svc.call("nostrconnect.reject", { id: offer.id }, function() { root.svc.refreshOffers() })
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
              picture: root.mode === "offer" ? (root.offer.image || "")
                : root.mode === "prompt" ? (root.prompt.app_image || "") : ""
            }
            Column {
              width: parent.width - Style.space(46)
              anchors.verticalCenter: parent.verticalCenter
              spacing: Style.space(2)
              Text {
                width: parent.width
                wrapMode: Text.Wrap
                color: root.foreground
                font.family: Style.font.family
                font.pixelSize: Style.font.heading
                font.bold: true
                text: {
                  switch (root.mode) {
                  case "unlock": return "Unlock Opal"
                  case "offer": return (root.offer.name || "An app") + " wants to connect"
                  case "prompt": return root.prompt.app_name
                  }
                  return ""
                }
              }
              Text {
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
                    return (root.offer.url || "") + (root.offer.url ? " · " : "") + root.offer.relays.join(", ")
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
              width: parent.width
              wrapMode: Text.Wrap
              color: root.foreground
              font.family: Style.font.family
              font.pixelSize: Style.font.body
              text: "It will sign as " + (root.svc && root.svc.currentAccount ? root.svc.currentAccount.label : "your account") + "."
            }
            PanelSectionHeader { text: "WHEN IT ASKS"; foreground: root.dim }
            ButtonGroup {
              width: parent.width
              options: U.policyOptions
              value: root.policy
              foreground: root.foreground
              onChanged: function(v) { root.policy = v }
            }
            PanelSectionHeader {
              visible: root.offer && root.offer.perms.length > 0
              text: "IT ASKS TO ALWAYS ALLOW"
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
              width: parent.width
              wrapMode: Text.Wrap
              color: root.foreground
              font.family: Style.font.family
              font.pixelSize: Style.font.title
              text: root.prompt ? U.describe(root.prompt.method, root.prompt.kind_label) : ""
            }
            Text {
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

            PanelSectionHeader { text: "REMEMBER THIS ANSWER"; foreground: root.dim }
            ButtonGroup {
              width: parent.width
              options: U.rememberOptions
              value: root.remember
              foreground: root.foreground
              onChanged: function(v) { root.remember = v }
            }
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
              text: root.mode === "unlock" ? "Unlock" : root.mode === "offer" ? "Connect" : "Approve"
              iconText: root.mode === "unlock" ? "󰿆" : "󰄬"
              iconSpinning: root.busy
              bordered: true
              active: true
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
