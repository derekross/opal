import QtQuick
import qs.Commons
import qs.Ui
import "util.js" as U

// Connected apps: create logins, list apps, and edit one app's permissions.
Column {
  id: root
  property var svc: null
  property color foreground: Color.foreground
  property color urgent: Color.urgent
  readonly property color dim: Qt.darker(foreground, 1.55)

  property string selectedId: ""
  property var detail: null          // { app, rules } from apps.get
  property bool confirmRevoke: false
  property string qrPath: ""
  property double nowMs: Date.now()

  spacing: Style.space(10)

  readonly property var apps: svc ? svc.apps : []

  function openApp(id) {
    selectedId = id
    confirmRevoke = false
    loadDetail()
  }
  function loadDetail() {
    if (!selectedId) return
    svc.call("apps.get", { id: selectedId }, function(err, r) {
      if (err) { root.selectedId = ""; root.detail = null; return }
      root.detail = r
    })
  }
  function createBunker() {
    svc.run("apps.create_bunker", { name: bunkerName.text.trim() || null, unused_ttl_secs: 3600 }, function(r) {
      bunkerName.text = ""
      svc.lastBunker = r
      svc.refreshApps()
      svc.call("qr.svg", { data: r.uri }, function(err, q) {
        if (!err) root.qrPath = q.data_url
      })
    })
  }
  function pasteConnect() {
    var uri = connectField.text.trim()
    if (uri.indexOf("nostrconnect://") !== 0) {
      svc.message("That isn't a nostrconnect:// link", true)
      return
    }
    svc.run("nostrconnect.offer", { uri: uri }, function() { connectField.text = "" })
  }

  Timer {
    interval: 30000
    running: root.visible
    repeat: true
    onTriggered: root.nowMs = Date.now()
  }

  Connections {
    target: root.svc
    function onAppsChanged() { if (root.selectedId) root.loadDetail() }
  }

  // ── List mode ────────────────────────────────────────────────────
  Column {
    width: parent.width
    visible: root.selectedId === ""
    spacing: Style.space(10)

    PanelSectionHeader { text: "LOG IN TO AN APP"; foreground: root.dim }

    Row {
      width: parent.width
      spacing: Style.space(8)
      TextField {
        id: bunkerName
        width: parent.width - newButton.width - parent.spacing
        placeholderText: "App name (optional)"
        foreground: root.foreground
        onAccepted: root.createBunker()
        Keys.onEscapePressed: focus = false
      }
      Button {
        id: newButton
        anchors.verticalCenter: bunkerName.verticalCenter
        text: "Bunker link"
        iconText: "󰐕"
        bordered: true
        foreground: root.foreground
        tooltipText: "Create a single-use bunker:// link to paste into the app"
        onClicked: root.createBunker()
      }
    }

    // The link just created.
    BorderSurface {
      width: parent.width
      visible: !!root.svc && !!root.svc.lastBunker
      color: "transparent"
      borderSpec: Border.surfaceSpec("popups", "border", Color.popups.border, Math.max(1, Style.space(1)))
      implicitHeight: bunkerCol.implicitHeight + Style.space(20)

      Column {
        id: bunkerCol
        anchors.fill: parent
        anchors.margins: Style.space(10)
        spacing: Style.space(8)

        Text {
          textFormat: Text.PlainText
          width: parent.width
          wrapMode: Text.Wrap
          color: root.foreground
          font.family: Style.font.family
          font.pixelSize: Style.font.bodySmall
          text: "Paste this into the app's \"bunker\" or \"remote signer\" login, or scan it. It works once and expires in an hour if unused."
        }
        Image {
          anchors.horizontalCenter: parent.horizontalCenter
          width: Style.space(180)
          height: width
          source: root.qrPath
          visible: root.qrPath !== ""
          sourceSize.width: 360
          sourceSize.height: 360
          smooth: false
        }
        Text {
          textFormat: Text.PlainText
          width: parent.width
          elide: Text.ElideMiddle
          color: root.dim
          font.family: Style.font.family
          font.pixelSize: Style.font.caption
          text: root.svc && root.svc.lastBunker ? root.svc.lastBunker.uri : ""
        }
        Row {
          spacing: Style.space(8)
          Button {
            text: "Copy link"
            iconText: "󰆏"
            bordered: true
            foreground: root.foreground
            onClicked: root.svc.copy(root.svc.lastBunker.uri, "Bunker link copied")
          }
          Button {
            text: "Done"
            foreground: root.foreground
            onClicked: { root.svc.lastBunker = null; root.qrPath = "" }
          }
        }
      }
    }

    Row {
      width: parent.width
      spacing: Style.space(8)
      TextField {
        id: connectField
        width: parent.width - connectButton.width - parent.spacing
        placeholderText: "…or paste a nostrconnect:// link"
        foreground: root.foreground
        onAccepted: root.pasteConnect()
        Keys.onEscapePressed: focus = false
      }
      Button {
        id: connectButton
        anchors.verticalCenter: connectField.verticalCenter
        text: "Connect"
        iconText: "󰌷"
        bordered: true
        foreground: root.foreground
        onClicked: root.pasteConnect()
      }
    }

    PanelSectionHeader {
      text: "APPS (" + root.apps.length + ")"
      foreground: root.dim
    }

    Text {
      textFormat: Text.PlainText
      width: parent.width
      visible: root.apps.length === 0
      wrapMode: Text.Wrap
      color: root.dim
      font.family: Style.font.family
      font.pixelSize: Style.font.bodySmall
      text: "No apps yet. Create a bunker link above, or open a nostrconnect:// link from an app."
    }

    Repeater {
      model: root.apps
      delegate: CursorSurface {
        required property var modelData
        width: root.width
        foreground: root.foreground
        implicitHeight: appRow.implicitHeight + Style.spacing.xl

        Row {
          id: appRow
          anchors.left: parent.left
          anchors.right: parent.right
          anchors.leftMargin: Style.space(8)
          anchors.rightMargin: Style.space(8)
          anchors.verticalCenter: parent.verticalCenter
          spacing: Style.space(10)

          Avatar {
            anchors.verticalCenter: parent.verticalCenter
            size: Style.space(28)
            picture: modelData.image || ""
            foreground: root.foreground
          }
          Column {
            width: parent.width - Style.space(38)
            anchors.verticalCenter: parent.verticalCenter
            spacing: Style.space(1)
            Text {
              textFormat: Text.PlainText
              width: parent.width
              elide: Text.ElideRight
              color: root.foreground
              font.family: Style.font.family
              font.pixelSize: Style.font.body
              text: modelData.display_name
            }
            Text {
              textFormat: Text.PlainText
              width: parent.width
              elide: Text.ElideRight
              color: root.dim
              font.family: Style.font.family
              font.pixelSize: Style.font.caption
              text: modelData.kind === "local"
                ? "Local app · " + (modelData.last_used ? "used " + U.ago(modelData.last_used, root.nowMs) : "not used yet") + " · " + modelData.policy
                : modelData.connected
                ? "Used " + U.ago(modelData.last_used, root.nowMs) + " · " + modelData.policy
                : "Waiting for the app to connect"
            }
          }
        }
        MouseArea {
          anchors.fill: parent
          hoverEnabled: true
          cursorShape: Qt.PointingHandCursor
          onClicked: root.openApp(modelData.id)
        }
      }
    }
  }

  // ── Detail mode ──────────────────────────────────────────────────
  Column {
    width: parent.width
    visible: root.selectedId !== "" && !!root.detail
    spacing: Style.space(10)

    readonly property var app: root.detail ? root.detail.app : ({})
    readonly property var rules: root.detail ? root.detail.rules : []

    Button {
      text: "All apps"
      iconText: "󰅁"
      foreground: root.foreground
      onClicked: { root.selectedId = ""; root.detail = null }
    }

    Text {
      textFormat: Text.PlainText
      width: parent.width
      wrapMode: Text.Wrap
      color: root.foreground
      font.family: Style.font.family
      font.pixelSize: Style.font.title
      font.bold: true
      text: parent.app.display_name || ""
    }
    Text {
      textFormat: Text.PlainText
      width: parent.width
      wrapMode: Text.Wrap
      color: root.dim
      font.family: Style.font.family
      font.pixelSize: Style.font.caption
      text: {
        var a = parent.app
        if (!a.id) return ""
        var parts = []
        if (a.kind === "local") {
          parts.push("A program on this computer")
          parts.push("program: " + (a.exe || "unknown"))
          parts.push("may ask for kinds " + (a.kinds || []).join(", ") + (a.nip44 ? " and its own encryption" : ""))
          return parts.join(" · ")
        }
        if (a.url) parts.push(a.url)
        parts.push(a.connected ? "connected" : "not connected yet")
        parts.push("relays: " + (a.relays || []).join(", "))
        return parts.join(" · ")
      }
    }

    PanelSectionHeader { text: "WHEN THIS APP ASKS"; foreground: root.dim }
    ButtonGroup {
      width: parent.width
      options: U.policyOptions
      value: parent.app.policy || "basic"
      foreground: root.foreground
      onChanged: function(v) {
        root.svc.runGuarded("apps.update", { id: root.selectedId, policy: v }, function() {
          root.svc.refreshApps()
          root.loadDetail()
        }, "Trusting an app fully lets it sign anything. Confirm with your Opal passphrase.")
      }
    }

    PanelSectionHeader {
      text: "SAVED ANSWERS (" + parent.rules.length + ")"
      foreground: root.dim
    }
    Text {
      textFormat: Text.PlainText
      width: parent.width
      visible: parent.rules.length === 0
      wrapMode: Text.Wrap
      color: root.dim
      font.family: Style.font.family
      font.pixelSize: Style.font.caption
      text: "Answers you ask Opal to remember show up here."
    }
    Repeater {
      model: parent.rules
      delegate: Item {
        required property var modelData
        width: root.width
        implicitHeight: Math.max(ruleText.implicitHeight, delButton.implicitHeight)

        Text {
          textFormat: Text.PlainText
          id: ruleText
          anchors.left: parent.left
          anchors.right: delButton.left
          anchors.verticalCenter: parent.verticalCenter
          elide: Text.ElideRight
          color: modelData.allow ? root.foreground : root.urgent
          font.family: Style.font.family
          font.pixelSize: Style.font.bodySmall
          text: (modelData.allow ? "Allow  " : "Deny  ")
            + U.describe(modelData.method, modelData.kind_label)
            + (modelData.kind === null && modelData.method === "sign_event" ? " (any kind)" : "")
            + (modelData.until ? "  · until " + U.clock(modelData.until) : "")
        }
        PanelActionButton {
          id: delButton
          anchors.right: parent.right
          anchors.verticalCenter: parent.verticalCenter
          iconText: "󰆴"
          tooltipText: "Forget this answer"
          onClicked: root.svc.run("apps.delete_rule",
            { id: root.selectedId, method: modelData.method, kind: modelData.kind },
            function() { root.loadDetail() })
        }
      }
    }

    Row {
      spacing: Style.space(8)
      Button {
        visible: parent.parent.rules.length > 0
        text: "Forget all"
        foreground: root.foreground
        onClicked: root.svc.run("apps.clear_rules", { id: root.selectedId }, function() { root.loadDetail() })
      }
      Button {
        text: root.confirmRevoke ? "Click again to revoke" : "Revoke access"
        iconText: "󰆴"
        bordered: true
        foreground: root.urgent
        onClicked: {
          if (!root.confirmRevoke) { root.confirmRevoke = true; return }
          root.svc.run("apps.remove", { id: root.selectedId }, function() {
            root.selectedId = ""
            root.detail = null
            root.svc.refreshApps()
          })
        }
      }
    }
  }
}
