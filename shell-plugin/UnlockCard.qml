import QtQuick
import qs.Commons
import qs.Ui

// Passphrase entry shown while the vault is locked.
Column {
  id: root
  property var svc: null
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
    svc.call("unlock", { passphrase: field.text }, function(err) {
      root.busy = false
      field.text = ""
      if (err) {
        root.error = err
        root.focusField()
      }
    })
  }

  onVisibleChanged: if (visible) focusField()

  Text {
    width: parent.width
    wrapMode: Text.Wrap
    color: root.foreground
    font.family: Style.font.family
    font.pixelSize: Style.font.body
    text: root.svc && root.svc.unlockRequest
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
      text: root.busy ? "Unlocking" : "Unlock"
      iconText: "󰿆"
      iconSpinning: root.busy
      bordered: true
      foreground: root.foreground
      onClicked: root.submit()
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
}
