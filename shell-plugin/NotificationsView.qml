import QtQuick
import Quickshell
import qs.Commons
import qs.Ui
import "util.js" as U

// Replies, mentions, reposts, reactions, zaps and DMs, newest first.
Column {
  id: root
  property var svc: null
  property color foreground: Color.foreground
  property color urgent: Color.urgent
  property bool panelOpen: false
  readonly property color dim: Qt.darker(foreground, 1.55)

  property string filter: "all"
  // Leave the DMs filter if it disappears.
  Connections {
    target: root.svc
    function onCanReadDmsChanged() { if (!root.svc.canReadDms && root.filter === "dms") root.filter = "all" }
  }
  property double nowMs: Date.now()

  spacing: Style.space(10)

  signal opened()

  readonly property var items: {
    var all = svc ? svc.notifications : []
    if (svc && !svc.canReadDms) all = all.filter(function(n) { return n.type !== "dm" })
    if (filter === "all") return all
    return all.filter(function(n) {
      if (filter === "replies") return n.type === "reply" || n.type === "mention"
      if (filter === "zaps") return n.type === "zap"
      if (filter === "dms") return n.type === "dm"
      return true
    })
  }
  readonly property var st: svc && svc.notifyStatus ? svc.notifyStatus : ({})

  function glyph(t) {
    return { reply: "󰑚", mention: "@", repost: "󰑖", reaction: "󰋑", zap: "󱐋", dm: "󰇮" }[t] || "󰂚"
  }
  function verb(n) {
    switch (n.type) {
    case "reply": return "replied"
    case "mention": return "mentioned you"
    case "repost": return "reposted"
    case "reaction": return "reacted " + n.detail
    case "zap": return "zapped " + (n.sats ? n.sats.toLocaleString() + " sats" : "you")
    case "dm": return "sent you a message"
    }
    return ""
  }
  function name(n) {
    return n.author_name || U.shortKey(n.author)
  }

  // Seeing the list counts as reading it.
  Timer {
    id: readTimer
    interval: 1500
    running: root.visible && root.panelOpen && !!root.svc && root.svc.unread > 0
    onTriggered: root.svc.call("notifications.mark_read", null)
  }
  Timer {
    interval: 30000
    running: root.visible
    repeat: true
    onTriggered: root.nowMs = Date.now()
  }

  Text {
    width: parent.width
    wrapMode: Text.Wrap
    color: root.dim
    font.family: Style.font.family
    font.pixelSize: Style.font.caption
    text: {
      if (!root.st.running) return "Starting…"
      var s = root.st.status || {}
      var parts = [(s.read_relays || []).length + " relays"]
      if (s.muted) parts.push(s.muted + " muted")
      if (root.svc && root.svc.canReadDms) parts.push(s.dms ? "DMs on" : "DMs off")
      return parts.join(" · ")
    }
  }

  ButtonGroup {
    width: parent.width
    options: [
      { value: "all", label: "All" },
      { value: "replies", label: "Replies" },
      { value: "zaps", label: "Zaps" },
      { value: "dms", label: "DMs" }
    ].filter(function(o) { return o.value !== "dms" || (!!root.svc && root.svc.canReadDms) })
    value: root.filter
    foreground: root.foreground
    onChanged: function(v) { root.filter = v }
  }

  Text {
    width: parent.width
    visible: root.items.length === 0
    wrapMode: Text.Wrap
    color: root.dim
    font.family: Style.font.family
    font.pixelSize: Style.font.bodySmall
    text: "Nothing yet. New replies, mentions, reactions, zaps and messages show up here."
  }

  Repeater {
    model: root.items.slice(0, 80)
    delegate: CursorSurface {
      required property var modelData
      width: root.width
      foreground: root.foreground
      current: modelData.unread
      implicitHeight: row.implicitHeight + Style.spacing.lg

      Row {
        id: row
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.leftMargin: Style.space(6)
        anchors.rightMargin: Style.space(6)
        anchors.verticalCenter: parent.verticalCenter
        spacing: Style.space(10)

        Item {
          width: Style.space(32)
          height: Style.space(32)
          Avatar {
            anchors.fill: parent
            size: parent.width
            picture: modelData.author_picture || ""
            foreground: root.foreground
          }
          Rectangle {
            anchors.right: parent.right
            anchors.bottom: parent.bottom
            anchors.margins: -Style.space(3)
            width: Style.space(16)
            height: width
            radius: width / 2
            color: Color.popups.background
            Text {
              anchors.centerIn: parent
              text: root.glyph(modelData.type)
              color: modelData.type === "zap" ? Color.accent : root.foreground
              font.family: Style.font.family
              font.pixelSize: Style.font.caption
            }
          }
        }

        Column {
          width: parent.width - Style.space(42)
          spacing: Style.space(2)
          Text {
            width: parent.width
            elide: Text.ElideRight
            color: root.foreground
            font.family: Style.font.family
            font.pixelSize: Style.font.bodySmall
            font.bold: modelData.unread
            text: root.name(modelData) + " " + root.verb(modelData)
          }
          Text {
            width: parent.width
            visible: text !== "" && modelData.type !== "reaction"
            wrapMode: Text.Wrap
            maximumLineCount: 3
            elide: Text.ElideRight
            color: root.foreground
            font.family: Style.font.family
            font.pixelSize: Style.font.bodySmall
            text: modelData.type === "dm" && !modelData.detail ? "" : (modelData.detail || "")
          }
          Text {
            width: parent.width
            visible: !!modelData.context
            elide: Text.ElideRight
            color: root.dim
            font.family: Style.font.family
            font.pixelSize: Style.font.caption
            text: "on: " + (modelData.context || "").replace(/\s+/g, " ")
          }
          Text {
            color: root.dim
            font.family: Style.font.family
            font.pixelSize: Style.font.caption
            text: U.ago(modelData.created_at, root.nowMs)
          }
        }
      }
      MouseArea {
        anchors.fill: parent
        enabled: !!modelData.url
        cursorShape: Qt.PointingHandCursor
        onClicked: {
          Quickshell.execDetached(["xdg-open", modelData.url])
          root.opened()
        }
      }
    }
  }

  Row {
    spacing: Style.space(8)
    visible: root.items.length > 0
    Button {
      text: "Mark all read"
      iconText: "󰄹"
      foreground: root.foreground
      onClicked: root.svc.run("notifications.mark_read", null)
    }
    Button {
      text: "Clear"
      foreground: root.dim
      onClicked: root.svc.run("notifications.clear", null, function() { root.svc.refreshNotifications() })
    }
  }
}
