import QtQuick
import Quickshell
import qs.Commons
import qs.Ui
import "util.js" as U

// Now playing, your status, and listening history.
Column {
  id: root
  property var svc: null
  property color foreground: Color.foreground
  property color urgent: Color.urgent
  readonly property color dim: Qt.darker(foreground, 1.55)

  readonly property var info: svc ? svc.statusInfo : ({})
  readonly property bool running: info.running === true
  readonly property var snap: info.snapshot || ({})
  readonly property var np: snap.now_playing || null
  readonly property var general: snap.general || null
  readonly property var stats: svc ? svc.playStats : ({})

  property string expiry: "never"
  property bool busy: false
  property double nowMs: Date.now()

  spacing: Style.space(10)

  function setStatus() {
    var text = statusField.text.trim()
    if (text === "") return
    var secs = { "1h": 3600, "4h": 14400, "today": secondsLeftToday(), "never": null }[expiry]
    busy = true
    svc.call("status.set", { text: text, link: linkField.text.trim() || null, expires_in: secs }, function(err) {
      root.busy = false
      if (err) { root.svc.message(err, true); return }
      statusField.text = ""; linkField.text = ""
      root.svc.refreshStatus()
    })
  }
  function secondsLeftToday() {
    var now = new Date()
    var end = new Date(now.getFullYear(), now.getMonth(), now.getDate(), 23, 59, 59)
    return Math.max(60, Math.floor((end - now) / 1000))
  }

  Timer {
    interval: 30000
    running: root.visible
    repeat: true
    onTriggered: root.nowMs = Date.now()
  }

  // ── Not running ────────────────────────────────────────────────
  Text {
    width: parent.width
    visible: !root.running
    wrapMode: Text.Wrap
    color: root.info.blocked ? root.urgent : root.dim
    font.family: Style.font.family
    font.pixelSize: Style.font.bodySmall
    text: root.info.blocked || "Starting…"
  }

  // ── Now playing ────────────────────────────────────────────────
  PanelSectionHeader { text: "NOW PLAYING"; foreground: root.dim; visible: root.running }
  Row {
    width: parent.width
    visible: root.running
    spacing: Style.space(10)
    Rectangle {
      width: Style.space(40)
      height: width
      radius: Style.space(4)
      color: Style.selectedFillFor(root.foreground, Color.accent)
      clip: true
      Image {
        anchors.fill: parent
        source: root.np && root.np.art_url && (root.np.art_url.indexOf("https://") === 0 || root.np.art_url.indexOf("file://") === 0) ? root.np.art_url : ""
        fillMode: Image.PreserveAspectCrop
        asynchronous: true
      }
      Text {
        anchors.centerIn: parent
        visible: !root.np || !root.np.art_url
        text: "󰝚"
        color: root.foreground
        font.family: Style.font.family
        font.pixelSize: Style.font.heading
      }
    }
    Column {
      width: parent.width - Style.space(50)
      anchors.verticalCenter: parent.verticalCenter
      spacing: Style.space(2)
      Text {
        width: parent.width
        elide: Text.ElideRight
        color: root.foreground
        font.family: Style.font.family
        font.pixelSize: Style.font.body
        text: root.np ? root.np.title : "Nothing playing"
      }
      Text {
        width: parent.width
        elide: Text.ElideRight
        color: root.dim
        font.family: Style.font.family
        font.pixelSize: Style.font.caption
        text: {
          if (!root.np) return root.svc && root.svc.config.status && root.svc.config.status.music === false ? "Music status is off" : "Play something in any MPRIS player"
          var parts = [root.np.artist, root.np.player]
          parts.push(root.snap.music ? "shared" : "not shared yet")
          return parts.filter(function(p) { return !!p }).join(" · ")
        }
      }
    }
  }

  // ── Your status ────────────────────────────────────────────────
  PanelSectionHeader { text: "YOUR STATUS"; foreground: root.dim; visible: root.running }
  Row {
    width: parent.width
    visible: root.running && !!root.general
    spacing: Style.space(8)
    Text {
      width: parent.width - clearButton.width - parent.spacing
      anchors.verticalCenter: parent.verticalCenter
      wrapMode: Text.Wrap
      color: root.foreground
      font.family: Style.font.family
      font.pixelSize: Style.font.body
      text: root.general
        ? root.general.content + (root.general.source !== "manual" ? "  (" + root.general.source + ")" : "")
          + (root.general.expires_at ? " · until " + U.clock(root.general.expires_at) : "")
        : ""
    }
    Button {
      id: clearButton
      visible: !!root.general && root.general.source === "manual"
      text: "Clear"
      foreground: root.foreground
      onClicked: root.svc.run("status.clear", null, function() { root.svc.refreshStatus() })
    }
  }
  TextField {
    id: statusField
    width: parent.width
    visible: root.running
    placeholderText: "What are you up to?"
    foreground: root.foreground
    onAccepted: root.setStatus()
    Keys.onEscapePressed: focus = false
  }
  TextField {
    id: linkField
    width: parent.width
    visible: root.running && statusField.text !== ""
    placeholderText: "Link (optional, https://…)"
    foreground: root.foreground
    onAccepted: root.setStatus()
  }
  Row {
    width: parent.width
    visible: root.running
    spacing: Style.space(8)
    ButtonGroup {
      width: parent.width - setButton.width - parent.spacing
      options: [
        { value: "1h", label: "1h" },
        { value: "4h", label: "4h" },
        { value: "today", label: "Today" },
        { value: "never", label: "Until cleared" }
      ]
      value: root.expiry
      foreground: root.foreground
      onChanged: function(v) { root.expiry = v }
    }
    Button {
      id: setButton
      anchors.verticalCenter: parent.verticalCenter
      text: root.busy ? "Posting…" : "Set"
      iconSpinning: root.busy
      bordered: true
      foreground: root.foreground
      onClicked: root.setStatus()
    }
  }

  // ── Listening ──────────────────────────────────────────────────
  PanelSectionHeader {
    visible: root.svc && root.svc.plays.length > 0
    text: "LISTENING · " + (root.stats.today || 0) + " today · " + (root.stats.week || 0) + " this week"
    foreground: root.dim
  }
  Flow {
    width: parent.width
    visible: !!root.stats.top_week && root.stats.top_week.length > 0
    spacing: Style.space(6)
    Repeater {
      model: root.stats.top_week || []
      delegate: Rectangle {
        required property var modelData
        radius: height / 2
        color: Style.selectedFillFor(root.foreground, Color.accent)
        implicitWidth: chip.implicitWidth + Style.space(16)
        implicitHeight: chip.implicitHeight + Style.space(6)
        Text {
          id: chip
          anchors.centerIn: parent
          color: root.foreground
          font.family: Style.font.family
          font.pixelSize: Style.font.caption
          text: modelData.name + " · " + modelData.plays
        }
      }
    }
  }
  Repeater {
    model: root.svc ? root.svc.plays.slice(0, 12) : []
    delegate: Item {
      required property var modelData
      width: root.width
      implicitHeight: playCol.implicitHeight + Style.space(4)
      Column {
        id: playCol
        width: parent.width
        spacing: Style.space(1)
        Text {
          width: parent.width
          elide: Text.ElideRight
          color: root.foreground
          font.family: Style.font.family
          font.pixelSize: Style.font.bodySmall
          text: modelData.title + (modelData.artist ? " — " + modelData.artist : "")
        }
        Text {
          color: root.dim
          font.family: Style.font.family
          font.pixelSize: Style.font.caption
          text: U.ago(modelData.played_at, root.nowMs) + " · " + modelData.player + (modelData.event_id ? " · published" : "")
        }
      }
      MouseArea {
        anchors.fill: parent
        enabled: !!modelData.link
        cursorShape: Qt.PointingHandCursor
        onClicked: Quickshell.execDetached(["xdg-open", modelData.link])
      }
    }
  }
}
