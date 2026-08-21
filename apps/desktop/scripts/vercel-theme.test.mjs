import assert from "node:assert/strict";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import { chromium } from "playwright-core";

const appDir = fileURLToPath(new URL("..", import.meta.url));
const chromePath = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";

const appearances = {
  light: {
    surface: ["#ffffff", "#ffffff", "#fafafa"],
    syntax: ["#c41562", "#107d32", "#0064e2", "#a64f00"],
    chart: ["rgb(0, 100, 226)", "rgb(124, 0, 201)", "rgb(16, 125, 50)", "rgb(166, 79, 0)"],
    border: ["#00000014", "#00000036"],
    selectionAlpha: 0.22,
    selectionStrongAlpha: 0.36,
    marqueeAlpha: 0.08,
    selectionBorder: "rgb(212, 212, 212)",
  },
  dark: {
    surface: ["#000000", "#000000", "#000000"],
    syntax: ["#f19baa", "#80cd82", "#8fbcec", "#eda661"],
    chart: ["rgb(80, 168, 255)", "rgb(196, 114, 251)", "rgb(0, 202, 82)", "rgb(255, 153, 0)"],
    border: ["#ffffff24", "#ffffff3d"],
    selectionAlpha: 0.22,
    selectionStrongAlpha: 0.36,
    marqueeAlpha: 0.08,
    selectionBorder: "rgb(51, 51, 51)",
  },
};

test("Vercel theme keeps surfaces, syntax, charts, and selection semantically distinct", async () => {
  const browser = await chromium.launch({ headless: true, executablePath: chromePath });
  try {
    const page = await browser.newPage();
    await page.setContent(`
      <main class="overviewPage">
        <i class="overviewTrendRung category0"></i>
        <i class="overviewTrendRung category1"></i>
        <i class="overviewTrendRung category2"></i>
        <i class="overviewTrendRung category3"></i>
      </main>
      <aside class="sidebar"></aside>
      <div class="configListPane"><div class="dataRow rowFrame configRowActive"></div></div>
      <div class="dataRow rowFrame rowSelected"></div>
      <div class="sessionListBody"><div class="dataRow rowFrame rowSelected sessionSelectedRow"></div></div>
      <div class="dataTableShell dataTableShell--frozen">
        <div class="dataRow rowFrame rowSelected dataRow--frozenPane"></div>
        <div class="dataRow rowFrame rowSelected dataRow--scrollPane"></div>
      </div>
      <div class="dataTableMarquee"></div>
      <button class="skillOpen skillDescriptionOpen dataCellSub">A long skill description that should be truncated in the table.</button>
      <span class="dataCellSub genericDataSub">A shared table description.</span>
      <div class="addSkillItem selected"></div>
      <div class="fileItem selected"></div>
      <div class="codeMirrorEditor">
        <div class="cm-selectionBackground"></div>
        <div class="cm-editor cm-focused"><div class="cm-scroller"><div class="cm-selectionLayer"><div class="cm-selectionBackground focusedSelection"></div></div></div></div>
      </div>
      <div class="segmentedControl visibility"><button class="segmentedControlItem segmentedSelection" data-state="on">Auto</button></div>
      <div class="modelConfigMarker"></div>
      <div class="modelConfigMarker transcriptTarget"></div>
      <button class="messageActionButton isCopied"></button>
      <span class="badge skillUpdateBadge" data-tone="warning">Update</span>
      <span class="badge skillWrapperBadge" data-tone="neutral">wrapper</span>
    `);
    for (const file of [
      "src/variables.css",
      "src/theme-overrides.css",
      "src/components/DataTable.css",
      "src/styles.css",
      "src/components/shared/Badge.css",
      "src/components/shared/SegmentedControl.css",
      "src/views/OverviewView.css",
      "src/views/ConfigView.css",
      "src/views/SessionsView.css",
    ]) {
      await page.addStyleTag({ path: `${appDir}/${file}` });
    }

    for (const [appearance, expected] of Object.entries(appearances)) {
      const actual = await page.evaluate((mode) => {
        const root = document.documentElement;
        root.dataset.theme = mode;
        root.dataset.colorTheme = "vercel";
        root.dataset.themeChanging = "true";
        root.style.colorScheme = mode;
        const rootStyle = getComputedStyle(root);
        const values = (names) => names.map((name) => rootStyle.getPropertyValue(name).trim());
        const resolveBorder = (variable) => {
          const probe = document.createElement("i");
          probe.style.border = `1px solid var(${variable})`;
          document.body.append(probe);
          const color = getComputedStyle(probe).borderColor;
          probe.remove();
          return color;
        };
        const chart = [0, 1, 2, 3].map((index) => (
          getComputedStyle(document.querySelector(`.category${index}`)).backgroundColor
        ));
        const selectionProbe = document.createElement("i");
        selectionProbe.style.background = "var(--selection-fill)";
        selectionProbe.style.border = "1px solid var(--selection-border)";
        document.body.append(selectionProbe);
        const selection = getComputedStyle(selectionProbe).backgroundColor;
        const selectionBorder = getComputedStyle(selectionProbe).borderColor;
        selectionProbe.style.background = "var(--selection-fill-strong)";
        const selectionStrong = getComputedStyle(selectionProbe).backgroundColor;
        selectionProbe.remove();
        const colorAlpha = (color) => {
          const slashAlpha = color.match(/\/\s*([\d.]+)\s*\)?$/);
          if (slashAlpha) return Number(slashAlpha[1]);
          const rgbaAlpha = color.match(/rgba\([^,]+,[^,]+,[^,]+,\s*([\d.]+)\)/);
          return rgbaAlpha ? Number(rgbaAlpha[1]) : 1;
        };
        const insetProbe = document.createElement("i");
        insetProbe.style.background = "var(--inset-fill)";
        document.body.append(insetProbe);
        const insetFill = getComputedStyle(insetProbe).backgroundColor;
        insetProbe.style.background = "var(--green-soft)";
        const greenFill = getComputedStyle(insetProbe).backgroundColor;
        insetProbe.remove();
        const activeConfigStyle = getComputedStyle(document.querySelector(".configRowActive"), "::after");
        const selectedRowStyle = getComputedStyle(document.querySelector(".rowSelected"), "::after");
        const sessionSelectedRowStyle = getComputedStyle(document.querySelector(".sessionSelectedRow"), "::after");
        const frozenSelectedRowStyle = getComputedStyle(document.querySelector(".dataRow--frozenPane"), "::after");
        const scrollSelectedRowStyle = getComputedStyle(document.querySelector(".dataRow--scrollPane"), "::after");
        const marqueeSelection = getComputedStyle(document.querySelector(".dataTableMarquee")).backgroundColor;
        const skillDescriptionStyle = getComputedStyle(document.querySelector(".skillDescriptionOpen"));
        const genericDataSubStyle = getComputedStyle(document.querySelector(".genericDataSub"));
        const modelConfigStyle = getComputedStyle(document.querySelector(".modelConfigMarker:not(.transcriptTarget)"));
        const transcriptTargetStyle = getComputedStyle(document.querySelector(".modelConfigMarker.transcriptTarget"));
        const updateBadge = document.querySelector(".skillUpdateBadge");
        const wrapperBadge = document.querySelector(".skillWrapperBadge");
        return {
          surface: values(["--theme-bg", "--theme-window", "--theme-sidebar"]),
          border: values(["--line", "--line-strong"]),
          borderConsumers: [
            "--line",
            "--control-border",
            "--row-frame-line",
            "--data-table-frozen-divider-color",
            "--image-outline",
          ].map(resolveBorder),
          sidebarBorder: getComputedStyle(document.querySelector(".sidebar")).borderRightColor,
          syntax: values(["--syntax-keyword", "--syntax-string", "--syntax-number", "--syntax-variable"]),
          chart,
          activeSelection: activeConfigStyle.backgroundColor,
          activeSelectionShadow: activeConfigStyle.boxShadow,
          selectedRow: selectedRowStyle.backgroundColor,
          selectedRowShadow: selectedRowStyle.boxShadow,
          sessionSelectedRow: sessionSelectedRowStyle.backgroundColor,
          sessionSelectedRowShadow: sessionSelectedRowStyle.boxShadow,
          frozenSelectedRowShadow: frozenSelectedRowStyle.boxShadow,
          scrollSelectedRowShadow: scrollSelectedRowStyle.boxShadow,
          selectedSkill: getComputedStyle(document.querySelector(".addSkillItem.selected")).backgroundColor,
          selectedFile: getComputedStyle(document.querySelector(".fileItem.selected")).backgroundColor,
          marqueeSelection,
          editorSelection: getComputedStyle(document.querySelector(".cm-selectionBackground")).backgroundColor,
          focusedEditorSelection: getComputedStyle(document.querySelector(".focusedSelection")).backgroundColor,
          segmentedSelection: getComputedStyle(document.querySelector(".segmentedSelection")).backgroundColor,
          modelConfig: modelConfigStyle.backgroundColor,
          copySuccess: getComputedStyle(document.querySelector(".messageActionButton.isCopied")).backgroundColor,
          transcriptTargetOutline: transcriptTargetStyle.outlineColor,
          insetFill,
          greenFill,
          updateBadge: {
            tone: updateBadge.dataset.tone,
            background: getComputedStyle(updateBadge).backgroundColor,
          },
          wrapperBadge: {
            tone: wrapperBadge.dataset.tone,
            background: getComputedStyle(wrapperBadge).backgroundColor,
          },
          selection,
          selectionStrong,
          selectionAlpha: colorAlpha(selection),
          selectionStrongAlpha: colorAlpha(selectionStrong),
          marqueeAlpha: colorAlpha(marqueeSelection),
          selectionBorder,
          skillDescription: {
            color: skillDescriptionStyle.color,
            minWidth: skillDescriptionStyle.minWidth,
            maxWidth: skillDescriptionStyle.maxWidth,
            overflow: skillDescriptionStyle.overflow,
            textOverflow: skillDescriptionStyle.textOverflow,
            whiteSpace: skillDescriptionStyle.whiteSpace,
          },
          genericDataSubColor: genericDataSubStyle.color,
        };
      }, appearance);

      assert.deepEqual(actual.surface, expected.surface, `${appearance} surfaces`);
      assert.deepEqual(actual.border, expected.border, `${appearance} official Vercel border scale`);
      assert.ok(actual.borderConsumers.every((color) => color === actual.borderConsumers[0]), `${appearance} default border consumers align`);
      assert.equal(actual.sidebarBorder, actual.borderConsumers[0], `${appearance} sidebar uses the default border`);
      assert.deepEqual(actual.syntax, expected.syntax, `${appearance} syntax`);
      assert.deepEqual(actual.chart, expected.chart, `${appearance} chart`);
      assert.equal(actual.selectionAlpha, expected.selectionAlpha, `${appearance} selection is translucent`);
      assert.equal(actual.selectionStrongAlpha, expected.selectionStrongAlpha, `${appearance} strong selection is translucent`);
      assert.equal(actual.marqueeAlpha, expected.marqueeAlpha, `${appearance} marquee selection is lighter than object selection`);
      assert.equal(actual.selectionBorder, expected.selectionBorder, `${appearance} selection border`);
      assert.equal(actual.activeSelection, actual.selection, `${appearance} active config selection`);
      assert.equal(actual.activeSelectionShadow, "none", `${appearance} active config has no outline`);
      assert.equal(actual.selectedRow, actual.selection, `${appearance} selected data row`);
      assert.equal(actual.selectedRowShadow, "none", `${appearance} selected data row has no outline`);
      assert.equal(actual.sessionSelectedRow, actual.selection, `${appearance} session active row uses the shared neutral fill`);
      assert.equal(actual.sessionSelectedRowShadow, "none", `${appearance} session active row has no outline`);
      assert.equal(actual.frozenSelectedRowShadow, "none", `${appearance} frozen selected row has no outline`);
      assert.equal(actual.scrollSelectedRowShadow, "none", `${appearance} scroll selected row has no outline`);
      assert.equal(actual.selectedSkill, actual.selection, `${appearance} selected skill`);
      assert.equal(actual.selectedFile, actual.selection, `${appearance} selected file`);
      assert.notEqual(actual.marqueeSelection, actual.selection, `${appearance} marquee uses a separate lighter fill`);
      assert.equal(actual.editorSelection, actual.selection, `${appearance} editor selection`);
      assert.equal(actual.focusedEditorSelection, actual.selectionStrong, `${appearance} focused editor selection uses strong fill`);
      assert.equal(actual.segmentedSelection, actual.selectionStrong, `${appearance} segmented selection uses strong fill`);
      assert.equal(actual.modelConfig, actual.insetFill, `${appearance} model config is neutral`);
      assert.notEqual(actual.modelConfig, actual.greenFill, `${appearance} model config is not success green`);
      assert.equal(actual.copySuccess, actual.greenFill, `${appearance} copy success stays green`);
      assert.equal(actual.updateBadge.tone, "warning", `${appearance} update badge is warning`);
      assert.equal(actual.wrapperBadge.tone, "neutral", `${appearance} wrapper badge is neutral`);
      assert.notEqual(actual.updateBadge.background, actual.wrapperBadge.background, `${appearance} update badge is more prominent than wrapper`);
      assert.equal(actual.transcriptTargetOutline, actual.selectionBorder, `${appearance} transcript target uses selection border`);
      assert.deepEqual(actual.skillDescription, {
        color: actual.genericDataSubColor,
        minWidth: "0px",
        maxWidth: "100%",
        overflow: "hidden",
        textOverflow: "ellipsis",
        whiteSpace: "nowrap",
      }, `${appearance} skill description truncates inside its button`);
    }
  } finally {
    await browser.close();
  }
});

test("all themes use the same object-selection roles", async () => {
  const browser = await chromium.launch({ headless: true, executablePath: chromePath });
  try {
    for (const colorTheme of ["sakura-pop", "dracula", "nord", "catppuccin", "tokyo-night", "gruvbox"]) {
      for (const mode of ["light", "dark"]) {
        const page = await browser.newPage();
        await page.setContent(`
          <div class="configListPane"><div class="dataRow rowFrame configRowActive"></div></div>
          <div class="dataRow rowFrame rowSelected genericSelected"></div>
          <div class="sessionListBody"><div class="dataRow rowFrame rowSelected sessionSelected"></div></div>
          <div class="dataTableShell dataTableShell--frozen">
            <div class="dataRow rowFrame rowSelected dataRow--frozenPane"></div>
            <div class="dataRow rowFrame rowSelected dataRow--scrollPane"></div>
          </div>
          <div class="dataTableMarquee"></div>
          <button class="skillOpen skillDescriptionOpen dataCellSub">A long skill description that should be truncated in the table.</button>
          <span class="dataCellSub genericDataSub">A shared table description.</span>
          <div class="addSkillItem selected"></div>
          <div class="fileItem selected"></div>
          <div class="codeMirrorEditor">
            <div class="cm-selectionBackground"></div>
            <div class="cm-editor cm-focused"><div class="cm-scroller"><div class="cm-selectionLayer"><div class="cm-selectionBackground focusedSelection"></div></div></div></div>
          </div>
          <div class="segmentedControl visibility"><button class="segmentedControlItem segmentedSelection" data-state="on">Auto</button></div>
        `);
        for (const file of [
          "src/variables.css",
          "src/theme-overrides.css",
          "src/components/DataTable.css",
          "src/styles.css",
          "src/components/shared/SegmentedControl.css",
          "src/views/ConfigView.css",
          "src/views/SessionsView.css",
        ]) {
          await page.addStyleTag({ path: `${appDir}/${file}` });
        }
        const actual = await page.evaluate(({ colorTheme: nextTheme, mode: nextMode }) => {
          const root = document.documentElement;
          root.dataset.theme = nextMode;
          root.dataset.colorTheme = nextTheme;
          root.dataset.themeChanging = "true";
          root.style.colorScheme = nextMode;
          const probe = document.createElement("i");
          probe.style.background = "var(--selection-fill)";
          document.body.append(probe);
          const selection = getComputedStyle(probe).backgroundColor;
          probe.style.background = "var(--selection-fill-strong)";
          const selectionStrong = getComputedStyle(probe).backgroundColor;
          probe.remove();
          const pseudo = (selector) => {
            const style = getComputedStyle(document.querySelector(selector), "::after");
            return { background: style.backgroundColor, shadow: style.boxShadow };
          };
          return {
            selection,
            selectionStrong,
            config: pseudo(".configRowActive"),
            row: pseudo(".genericSelected"),
            session: pseudo(".sessionSelected"),
            frozen: pseudo(".dataRow--frozenPane"),
            scroll: pseudo(".dataRow--scrollPane"),
            skill: getComputedStyle(document.querySelector(".addSkillItem.selected")).backgroundColor,
            file: getComputedStyle(document.querySelector(".fileItem.selected")).backgroundColor,
            editor: getComputedStyle(document.querySelector(".cm-selectionBackground")).backgroundColor,
            focusedEditor: getComputedStyle(document.querySelector(".focusedSelection")).backgroundColor,
            segmented: getComputedStyle(document.querySelector(".segmentedSelection")).backgroundColor,
            marquee: getComputedStyle(document.querySelector(".dataTableMarquee")).backgroundColor,
            skillDescription: (() => {
              const style = getComputedStyle(document.querySelector(".skillDescriptionOpen"));
              return {
                color: style.color,
                minWidth: style.minWidth,
                maxWidth: style.maxWidth,
                overflow: style.overflow,
                textOverflow: style.textOverflow,
                whiteSpace: style.whiteSpace,
              };
            })(),
            genericDataSubColor: getComputedStyle(document.querySelector(".genericDataSub")).color,
          };
        }, { colorTheme, mode });
        const label = `${colorTheme}/${mode}`;
        for (const [name, value] of Object.entries({
          config: actual.config.background,
          row: actual.row.background,
          session: actual.session.background,
          skill: actual.skill,
          file: actual.file,
          editor: actual.editor,
        })) {
          assert.equal(value, actual.selection, `${label} ${name} uses selection fill`);
        }
        assert.equal(actual.selectionStrong, actual.focusedEditor, `${label} focused editor uses strong fill`);
        assert.equal(actual.segmented, actual.selectionStrong, `${label} segmented selection uses strong fill`);
        assert.notEqual(actual.marquee, actual.selection, `${label} marquee uses a separate lighter fill`);
        assert.deepEqual(actual.skillDescription, {
          color: actual.genericDataSubColor,
          minWidth: "0px",
          maxWidth: "100%",
          overflow: "hidden",
          textOverflow: "ellipsis",
          whiteSpace: "nowrap",
        }, `${label} skill description truncates inside its button`);
        for (const name of ["config", "row", "session", "frozen", "scroll"]) {
          assert.equal(actual[name].shadow, "none", `${label} ${name} selection has no border`);
        }
        await page.close();
      }
    }
  } finally {
    await browser.close();
  }
});
