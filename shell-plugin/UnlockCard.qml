import QtQuick
import qs.Commons
import qs.Ui

// Passphrase entry: unlocks the vault, or, when it is already unlocked but
// this shell hasn't shown the passphrase yet (it restarted), signs in so
// requests and apps are visible again.
Column {
  id: root
  property var svc: null
  readonly property bool signIn: !!svc && !svc.locked
  property color foreground: Color.foreground
  property color urgent: Color.urgent
  property bool busy: false
  property string error: ""

  spacing: Style.space(8)

  function focusField() { Qt.callLater(function() { field.forceActiveFocus() }) }

  function submit() {
    if (busy || field.text === "") return
    busy = true
    error = ""
    var done = function(err) {
      root.busy = false
      field.text = ""
      if (err) {
        root.error = err
        root.focusField()
      }
    }
    if (signIn) svc.signIn(field.text, done)
    else svc.call("unlock", { passphrase: field.text }, done)
  }

  onVisibleChanged: if (visible) focusField()

  Text {
    textFormat: Text.PlainText
    width: parent.width
    wrapMode: Text.Wrap
    color: root.foreground
    font.family: Style.font.family
    font.pixelSize: Style.font.body
    text: root.signIn
      ? "Opal is unlocked, but this panel needs your passphrase to show requests and apps again."
      : root.svc && root.svc.unlockRequest
      ? root.svc.unlockRequest.app_name + " is waiting. Unlock to continue."
      : "Unlock to sign with your keys."
  }

  Row {
    width: parent.width
    spacing: Style.space(8)
    TextField {
      id: field
      width: parent.width - unlockButton.width - parent.spacing
      password: true
      placeholderText: "Opal passphrase"
      foreground: root.foreground
      enabled: !root.busy
      onAccepted: root.submit()
      Keys.onEscapePressed: field.focus = false
    }
    Button {
      id: unlockButton
      anchors.verticalCenter: field.verticalCenter
      text: root.busy ? (root.signIn ? "Signing in" : "Unlocking") : (root.signIn ? "Sign in" : "Unlock")
      iconText: "󰿆"
      iconSpinning: root.busy
      bordered: true
      foreground: root.foreground
      onClicked: root.submit()
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
}
