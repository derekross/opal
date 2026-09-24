import QtQuick
import QtQuick.Controls
import Quickshell
import Quickshell.Hyprland
import qs.Commons
import qs.Ui

// Bar icon + popup. One instance per monitor; all state lives in OpalService.
Panel {
  id: root
  moduleName: "opal"
  manageIpc: false

  // The service loads once for the whole shell; it may appear after us.
  property var svc: null
  function findService() {
    if (!svc && bar && bar.shell && typeof bar.shell.serviceFor === "function")
      svc = bar.shell.serviceFor("opal")
  }
  Component.onCompleted: findService()
  onBarChanged: findService()
  Timer {
    interval: 500
    running: !root.svc
    repeat: true
    onTriggered: root.findService()
  }

  // `omarchy-shell opal panel` (e.g. from a keybinding) toggles the popup on
  // the focused monitor.
  Connections {
    target: root.svc
    function onPanelToggleRequested() {
      var win = root.QsWindow.window
      var focused = Hyprland.focusedMonitor
      if (!win || !win.screen || !focused || win.screen.name === focused.name) root.toggle()
    }
  }

  readonly property color foreground: bar ? bar.foreground : Color.foreground
  readonly property color urgent: bar ? bar.urgent : Color.urgent

  readonly property bool daemonUp: !!svc && svc.connected
  readonly property bool locked: !svc || svc.locked
  readonly property int attention: svc ? svc.attentionCount : 0

  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  onOpenedChanged: {
    if (opened) {
      if (svc) svc.refreshAll()
      Qt.callLater(function() { content.focusDefault() })
    }
  }

  BarIconButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    text: "󰇈"
    active: root.attention > 0
    dimmed: !root.daemonUp || root.locked
    tooltipText: {
      if (!root.daemonUp) return "Opal isn't running"
      if (root.attention > 0) return root.attention + " waiting for you"
      if (!root.svc.hasAccounts) return "Opal: set up your Nostr key"
      return root.locked ? "Opal: locked" : "Opal: unlocked"
    }
    onPressed: function(buttonCode) {
      if (buttonCode === Qt.RightButton && root.svc) {
        // Right click: lock right away (or open the approvals if waiting).
        if (root.attention > 0) root.svc.showApproval()
        else root.svc.call("lock", null)
      } else {
        root.toggle()
      }
    }
  }

  KeyboardPanel {
    id: panel
    anchorItem: button
    owner: root
    bar: root.bar
    open: root.opened
    focusTarget: content.keyTarget
    contentWidth: panel.fittedContentWidth(Style.space(420))
    contentHeight: panel.fittedContentHeight(content.implicitHeight, Style.space(640))

    OpalPanel {
      id: content
      anchors.fill: parent
      svc: root.svc
      foreground: root.foreground
      urgent: root.urgent
      onCloseRequested: root.close()
    }
  }
}
