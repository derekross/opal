import QtQuick
import QtQuick.Controls
import Quickshell
import Quickshell.Hyprland
import Quickshell.Io
import qs.Commons
import qs.Ui

// Bar icon + popup. One instance per monitor; all state lives in OpalService.
Panel {
  id: root
  manageIpc: false

  // The service loads once for the whole shell; it may appear after us.
  property var svc: null
  function findService() {
    if (!svc && bar && bar.shell && typeof bar.shell.serviceFor === "function")
      svc = bar.shell.serviceFor(root.moduleName || "derekross.opal")
  }
  Component.onCompleted: { findService(); checkApproval() }
  onBarChanged: findService()
  Timer {
    interval: 500
    running: !root.svc
    repeat: true
    onTriggered: root.findService()
  }

  // ── Third-party bars ─────────────────────────────────────────
  // The shell hands plugin services only to its built-in bar. On a custom
  // bar serviceFor() stays null, so (with the user's OK) the widget runs its
  // own OpalService, which talks to opald directly.
  property bool serviceBlocked: false   // no shell service after a grace period
  property bool approved: false
  property bool approvalChecked: false
  readonly property string approvalFile:
    (Quickshell.env("XDG_STATE_HOME") || (Quickshell.env("HOME") + "/.local/state")) + "/opal/third-party-bar-approved"
  readonly property bool needsApproval: serviceBlocked && approvalChecked && !approved

  Timer {
    interval: 3000
    running: !root.svc && !root.serviceBlocked
    onTriggered: if (!root.svc) root.serviceBlocked = true
  }
  Process {
    id: checkProc
    command: ["/usr/bin/test", "-f", root.approvalFile]
    onExited: function(code) { root.approved = code === 0; root.approvalChecked = true }
  }
  Process {
    id: approveProc
    onExited: function(code) { if (code === 0) root.approved = true }
  }
  function checkApproval() { checkProc.running = true }
  function approve() {
    approveProc.command = ["/usr/bin/sh", "-c",
      'umask 077 && /usr/bin/mkdir -p "$(/usr/bin/dirname "$1")" && /usr/bin/date +%s > "$1"',
      "sh", root.approvalFile]
    approveProc.running = true
  }
  Loader {
    active: root.serviceBlocked && root.approved
    sourceComponent: OpalService { ipcEnabled: false }
    onLoaded: if (!root.svc) root.svc = item
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
      if (root.needsApproval) return "Opal: click to allow it on this custom bar"
      if (root.svc && root.svc.keyringLocked) return "Opal: waiting for your keyring to be unlocked"
      if (!root.daemonUp) return "Opal isn't running"
      if (root.attention > 0) return root.attention + " waiting for you"
      if (!root.svc.hasAccounts) return "Opal: set up your Nostr key"
      if (root.svc.unread > 0) return root.svc.unread + " new on Nostr"
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

  // Unread notifications: a small dot on the gem (approvals turn it red instead).
  Rectangle {
    visible: !!root.svc && root.svc.unread > 0 && root.attention === 0
    width: Style.space(6)
    height: width
    radius: width / 2
    color: Color.accent
    anchors.right: button.right
    anchors.top: button.top
    anchors.rightMargin: Style.space(5)
    anchors.topMargin: Style.space(4)
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
      needsApproval: root.needsApproval
      onApproveRequested: root.approve()
      foreground: root.foreground
      urgent: root.urgent
      panelOpen: root.opened
      onCloseRequested: root.close()
    }
  }
}
