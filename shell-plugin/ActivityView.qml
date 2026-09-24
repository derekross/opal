import QtQuick
import qs.Commons
import qs.Ui
import "util.js" as U

// What apps did with your keys, newest first.
Column {
  id: root
  property var svc: null
  property color foreground: Color.foreground
  property color urgent: Color.urgent
  readonly property color dim: Qt.darker(foreground, 1.55)

  property string filter: "all"

  spacing: Style.space(10)

  readonly property var entries: {
    var all = svc ? svc.activity : []
    if (filter === "all") return all
    var want = filter === "allowed"
    return all.filter(function(e) { return e.allowed === want })
  }
  readonly property var day: svc && svc.stats.day ? svc.stats.day : { allowed: 0, denied: 0 }
  readonly property var week: svc && svc.stats.week ? svc.stats.week : { allowed: 0, denied: 0 }

  Text {
    width: parent.width
    color: root.dim
    font.family: Style.font.family
    font.pixelSize: Style.font.caption
    text: "24h: " + root.day.allowed + " allowed, " + root.day.denied + " denied   ·   7d: "
      + root.week.allowed + " allowed, " + root.week.denied + " denied"
  }

  ButtonGroup {
    width: parent.width
    options: [
      { value: "all", label: "All" },
      { value: "allowed", label: "Allowed" },
      { value: "denied", label: "Denied" }
    ]
    value: root.filter
    foreground: root.foreground
    onChanged: function(v) { root.filter = v }
  }

  Text {
    width: parent.width
    visible: root.entries.length === 0
    wrapMode: Text.Wrap
    color: root.dim
    font.family: Style.font.family
    font.pixelSize: Style.font.bodySmall
    text: root.svc && root.svc.config.signer && root.svc.config.signer.privacy_mode
      ? "Privacy mode is on: nothing is recorded."
      : "Nothing yet."
  }

  Repeater {
    model: root.entries.slice(0, 100)
    delegate: Item {
      required property var modelData
      width: root.width
      implicitHeight: entryCol.implicitHeight + Style.space(6)

      Rectangle {
        id: dot
        width: Style.space(6)
        height: width
        radius: width / 2
        anchors.left: parent.left
        anchors.top: parent.top
        anchors.topMargin: Style.space(6)
        color: modelData.allowed ? Color.accent : root.urgent
      }
      Column {
        id: entryCol
        anchors.left: dot.right
        anchors.leftMargin: Style.space(8)
        anchors.right: parent.right
        spacing: Style.space(1)
        Text {
          width: parent.width
          elide: Text.ElideRight
          color: root.foreground
          font.family: Style.font.family
          font.pixelSize: Style.font.bodySmall
          text: modelData.app_name + " · " + U.describe(modelData.method, modelData.kind_label)
        }
        Text {
          width: parent.width
          elide: Text.ElideRight
          color: root.dim
          font.family: Style.font.family
          font.pixelSize: Style.font.caption
          text: U.clock(modelData.at) + " · " + (modelData.allowed ? "allowed" : "denied")
            + " by " + (U.sourceLabels[modelData.source] || modelData.source)
            + (modelData.reason && !modelData.allowed ? " · " + modelData.reason : "")
        }
      }
    }
  }

  Button {
    visible: root.entries.length > 0
    text: "Clear history"
    foreground: root.dim
    onClicked: root.svc.run("activity.clear", null, function() { root.svc.refreshActivity() })
  }
}
