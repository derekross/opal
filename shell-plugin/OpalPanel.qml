import QtQuick
import QtQuick.Controls
import Quickshell
import qs.Commons
import qs.Ui
import "util.js" as U

// Popup content: header (current profile + lock), then setup / unlock /
// tabs depending on the daemon's state.
Item {
  id: root

  property var svc: null
  property color foreground: Color.foreground
  property color urgent: Color.urgent
  readonly property color dim: Qt.darker(foreground, 1.55)
  readonly property string fontFamily: Style.font.family

  signal closeRequested()

  property alias keyTarget: keyCatcher
  property string tab: "apps"
  property string toast: ""
  property bool toastError: false

  readonly property bool up: !!svc && svc.connected
  readonly property bool editing: {
    var f = Window.activeFocusItem
    return !!f && f.echoMode !== undefined
  }

  implicitHeight: column.implicitHeight

  function focusDefault() {
    if (up && svc.hasAccounts && svc.locked) unlockCard.focusField()
    else keyCatcher.forceActiveFocus()
  }

  function showToast(text, isError) {
    toast = text
    toastError = isError
    toastTimer.restart()
  }

  Connections {
    target: root.svc
    function onMessage(text, isError) { root.showToast(text, isError) }
    function onTabRequested(name) { root.tab = name }
  }

  Timer {
    id: toastTimer
    interval: 3500
    onTriggered: root.toast = ""
  }

  property bool panelOpen: false
  readonly property bool notificationsOn: up && svc.notificationsOn
  readonly property var tabs: {
    var t = []
    if (notificationsOn) t.push({ value: "notifications", label: svc.unread > 0 ? "Inbox " + svc.unread : "Inbox" })
    if (up && svc.statusOn) t.push({ value: "status", label: "Status" })
    if (up && svc.hasAccounts) {
      t.push({ value: "apps", label: "Apps" })
      t.push({ value: "activity", label: "Activity" })
    }
    t.push({ value: "profiles", label: "Profiles" })
    t.push({ value: "settings", label: "Settings" })
    return t
  }
  onTabsChanged: {
    for (var i = 0; i < tabs.length; i++) if (tabs[i].value === tab) return
    tab = tabs.length > 0 ? tabs[0].value : "apps"
  }
  onNotificationsOnChanged: if (notificationsOn && tab === "apps") tab = "notifications"

  PanelKeyCatcher {
    id: keyCatcher
    anchors.fill: parent
    blocked: root.editing
    onCloseRequested: root.closeRequested()
    onMoveRequested: function(dx, dy) {
      if (dx === 0) return
      var i = 0
      for (; i < root.tabs.length; i++) if (root.tabs[i].value === root.tab) break
      root.tab = root.tabs[(i + dx + root.tabs.length) % root.tabs.length].value
    }

    Flickable {
      id: flick
      anchors.fill: parent
      contentWidth: width
      contentHeight: column.implicitHeight
      clip: true
      boundsBehavior: Flickable.StopAtBounds
      interactive: contentHeight > height
      ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }

      Column {
        id: column
        width: flick.width
        spacing: Style.space(12)

        // ── Header ────────────────────────────────────────────────
        Item {
          width: parent.width
          implicitHeight: Math.max(avatar.height, headerText.implicitHeight)

          Avatar {
            id: avatar
            anchors.left: parent.left
            anchors.verticalCenter: parent.verticalCenter
            size: Style.space(40)
            picture: !root.svc ? ""
              : root.svc.currentAccount ? (root.svc.currentAccount.picture || "")
              : (root.svc.status.watched && root.svc.status.watched.picture) || ""
            foreground: root.foreground
          }

          Column {
            id: headerText
            anchors.left: avatar.right
            anchors.leftMargin: Style.space(10)
            anchors.right: lockButton.left
            anchors.rightMargin: Style.space(8)
            anchors.verticalCenter: parent.verticalCenter
            spacing: Style.space(2)

            Text {
              width: parent.width
              elide: Text.ElideRight
              color: root.foreground
              font.family: root.fontFamily
              font.pixelSize: Style.font.heading
              font.bold: true
              text: {
                if (!root.up) return "Opal"
                var a = root.svc.currentAccount
                if (a) return a.label
                var w = root.svc.status.watched
                return root.svc.readOnly ? ((w && w.name) || "Watching") : "Opal"
              }
            }
            Text {
              width: parent.width
              elide: Text.ElideMiddle
              color: root.dim
              font.family: root.fontFamily
              font.pixelSize: Style.font.caption
              text: {
                if (!root.up) return "The Opal daemon isn't running"
                var a = root.svc.currentAccount
                if (!a) return root.svc.readOnly ? (root.svc.identity.nip05 || U.shortKey(root.svc.identity.npub)) + " · read-only" : "Nostr signer"
                return (root.svc.locked ? "Locked · " : "Unlocked · ") + U.shortKey(a.npub)
              }
              MouseArea {
                anchors.fill: parent
                enabled: root.up && !!root.svc.currentAccount
                cursorShape: Qt.PointingHandCursor
                onClicked: root.svc.copy(root.svc.currentAccount.npub, "npub copied")
              }
            }
          }

          Button {
            id: lockButton
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            visible: root.up && root.svc.hasAccounts
            iconText: root.up && root.svc.locked ? "󰌾" : "󰿆"
            tooltipText: root.up && root.svc.locked ? "Locked" : "Lock now"
            foreground: root.foreground
            onClicked: {
              if (!root.svc.locked) root.svc.run("lock")
              else unlockCard.focusField()
            }
          }
        }

        // ── Toast ─────────────────────────────────────────────────
        Text {
          width: parent.width
          visible: root.toast !== ""
          wrapMode: Text.Wrap
          color: root.toastError ? root.urgent : Color.accent
          font.family: root.fontFamily
          font.pixelSize: Style.font.bodySmall
          text: root.toast
        }

        // ── Daemon not running ────────────────────────────────────
        Column {
          width: parent.width
          visible: !root.up
          spacing: Style.space(8)
          Text {
            width: parent.width
            wrapMode: Text.Wrap
            color: root.dim
            font.family: root.fontFamily
            font.pixelSize: Style.font.body
            text: "Start the signer service to manage your keys."
          }
          Button {
            text: "Start Opal"
            iconText: "󰐊"
            bordered: true
            foreground: root.foreground
            onClicked: root.svc ? root.svc.startDaemon() : Quickshell.execDetached(["systemctl", "--user", "start", "opal.service"])
          }
        }

        // ── First run ─────────────────────────────────────────────
        SetupView {
          width: parent.width
          visible: root.up && !root.svc.configured
          svc: root.svc
          foreground: root.foreground
        }

        // ── Unlock ────────────────────────────────────────────────
        UnlockCard {
          id: unlockCard
          width: parent.width
          visible: root.up && root.svc.hasAccounts && root.svc.locked
          svc: root.svc
          foreground: root.foreground
          urgent: root.urgent
        }

        // ── Waiting requests ──────────────────────────────────────
        Button {
          width: parent.width
          visible: root.up && root.svc.attentionCount > 0
          leftAlign: true
          bordered: true
          iconText: "󰀦"
          foreground: root.urgent
          text: root.up ? (root.svc.attentionCount + " waiting for your approval") : ""
          onClicked: root.svc.showApproval()
        }

        // ── Tabs ──────────────────────────────────────────────────
        ButtonGroup {
          width: parent.width
          visible: root.up && root.svc.configured
          options: root.tabs
          value: root.tab
          foreground: root.foreground
          onChanged: function(v) { root.tab = v }
        }

        NotificationsView {
          width: parent.width
          visible: root.up && root.svc.configured && root.tab === "notifications"
          svc: root.svc
          foreground: root.foreground
          urgent: root.urgent
          panelOpen: root.panelOpen
          onOpened: root.closeRequested()
        }
        StatusView {
          width: parent.width
          visible: root.up && root.svc.configured && root.tab === "status"
          svc: root.svc
          foreground: root.foreground
          urgent: root.urgent
        }
        AppsView {
          width: parent.width
          visible: root.up && root.svc.hasAccounts && root.tab === "apps"
          svc: root.svc
          foreground: root.foreground
          urgent: root.urgent
        }
        ActivityView {
          width: parent.width
          visible: root.up && root.svc.hasAccounts && root.tab === "activity"
          svc: root.svc
          foreground: root.foreground
          urgent: root.urgent
        }
        ProfilesView {
          width: parent.width
          visible: root.up && root.svc.configured && root.tab === "profiles"
          svc: root.svc
          foreground: root.foreground
          urgent: root.urgent
        }
        SettingsView {
          width: parent.width
          visible: root.up && root.svc.configured && root.tab === "settings"
          svc: root.svc
          foreground: root.foreground
          urgent: root.urgent
        }
      }
    }
  }
}
