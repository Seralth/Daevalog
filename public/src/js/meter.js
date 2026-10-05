const createMeterUI = ({
  elList,
  dpsFormatter,
  getUserName,
  onClickUserRow,
  onHoverUserRow,
  onLeaveUserRow,
  getMetric,
  getSortDirection,
  getPinUserToTop,
  getPlayerLimit,
  // Column keys for the rows (see meterColumns.js), or null for the usual
  // readout. With them: the meter's number formats and the header labels.
  getColumns,
  columnFormats,
  getColumnLabel,
}) => {
  const Columns = window.MeterColumns;
  const MAX_CACHE = 32;
  const cjkRegex = /[\u3400-\u9FFF\uF900-\uFAFF]/;
  const classIconSrcByJob = new Map();

  const rowViewById = new Map();
  let lastVisibleIds = new Set();
  let pendingRenderRows = null;
  let renderRowsRafId = 0;

  const nowMs = () => (typeof performance !== "undefined" ? performance.now() : Date.now());

  const createRowView = (id) => {
    const rowEl = document.createElement("div");
    rowEl.className = "item";
    rowEl.style.display = "none";
    rowEl.dataset.rowId = String(id);

    // The bar spans the full row, so 100% reads as 100%. The readout keeps
    // its own scrim (see .dps in styles.css) for where the bar passes behind.
    const fillTrackEl = document.createElement("div");
    fillTrackEl.className = "fillTrack";

    const fillEl = document.createElement("div");
    fillEl.className = "fill";
    fillTrackEl.appendChild(fillEl);

    const contentEl = document.createElement("div");
    contentEl.className = "content";

    // Ladder position by damage. Kept separate from row order so that
    // "pin me to top" moves the row without misreporting the rank.
    const rankEl = document.createElement("span");
    rankEl.className = "rank";

    const classIconEl = document.createElement("div");
    classIconEl.className = "classIcon";

    const classIconImg = document.createElement("img");
    classIconImg.className = "classIconImg";
    classIconImg.style.visibility = "hidden";

    classIconImg.draggable = false;

    classIconEl.appendChild(classIconImg);

    const nameEl = document.createElement("div");
    nameEl.className = "name";

    // Party combat power, shown beside the name. Hidden unless the party roster
    // packet supplied a value for this player.
    const combatPowerEl = document.createElement("span");
    combatPowerEl.className = "combatPower";
    combatPowerEl.style.display = "none";

    const dpsContainer = document.createElement("div");
    const dpsNumber = document.createElement("p");
    dpsContainer.className = "dps";
    const dpsContribution = document.createElement("p");
    dpsContribution.className = "dpsContribution";

    dpsContainer.appendChild(dpsNumber);
    dpsContainer.appendChild(dpsContribution);

    contentEl.appendChild(rankEl);
    contentEl.appendChild(classIconEl);
    contentEl.appendChild(nameEl);
    contentEl.appendChild(combatPowerEl);
    contentEl.appendChild(dpsContainer);
    rowEl.appendChild(fillTrackEl);
    rowEl.appendChild(contentEl);

    const view = {
      id,
      rowEl,
      prevContribClass: "",
      nameEl,
      combatPowerEl,
      rankEl,
      dpsContainer,
      classIconEl,
      classIconImg,
      dpsNumber,
      dpsContribution,
      fillEl,
      currentRow: null,
      lastSeenAt: 0,
      isVisible: false,
      lastNameText: "",
      lastCombatPowerText: "",
      lastIsCjk: false,
      lastMetricText: "",
      lastContributionText: "",
      lastRankText: "",
      lastFillRatio: -1,
      lastClassIconSrc: "",
      lastIsUser: false,
      lastIsIdentifying: false,
      colsKey: "",
      colEls: [],
      colTexts: [],
      hoverRipplePlayed: false,
      hoverRippleTimer: null,
    };

    rowEl.addEventListener("mouseenter", (event) => {
      if (!view.hoverRipplePlayed) {
        view.rowEl.classList.add("hoverRippleOnce");
        view.hoverRipplePlayed = true;
        if (view.hoverRippleTimer) {
          clearTimeout(view.hoverRippleTimer);
        }
        view.hoverRippleTimer = setTimeout(() => {
          view.rowEl.classList.remove("hoverRippleOnce");
          view.hoverRippleTimer = null;
        }, 950);
      }
      onHoverUserRow?.(view.currentRow, event);
    });


    rowEl.addEventListener("mousemove", (event) => {
      if (view._lastMoveMs && nowMs() - view._lastMoveMs < 100) return;
      view._lastMoveMs = nowMs();
      onHoverUserRow?.(view.currentRow, event);
    });
    rowEl.addEventListener("mouseleave", () => {
      view.hoverRipplePlayed = false;
      if (view.hoverRippleTimer) {
        clearTimeout(view.hoverRippleTimer);
        view.hoverRippleTimer = null;
      }
      view.rowEl.classList.remove("hoverRippleOnce");
      onLeaveUserRow?.(view.currentRow);
    });

    rowEl.addEventListener("click", () => {
      // if (view.currentRow?.isUser)
      onClickUserRow?.(view.currentRow);
    });

    return view;
  };

  const getRowView = (id) => {
    let view = rowViewById.get(id);
    if (!view) {
      view = createRowView(id);
      rowViewById.set(id, view);
      elList.appendChild(view.rowEl);
    }
    return view;
  };

  const getDisplayRows = (sortedAll) => {
    const limit = (typeof getPlayerLimit === "function" ? getPlayerLimit() : 6) || 6;
    const topN = sortedAll.slice(0, limit);
    const user = sortedAll.find((x) => x.isUser);

    if (!user) return topN;
    const pinUser = typeof getPinUserToTop === "function" && getPinUserToTop();
    if (pinUser) {
      return [user, ...topN.filter((row) => !row.isUser)];
    }
    if (topN.some((x) => x.isUser)) return topN;
    return [...topN, user];
  };

  const pruneCache = (keepIds) => {
    if (rowViewById.size <= MAX_CACHE) return;

    const candidates = [];
    for (const [id, view] of rowViewById) {
      if (keepIds.has(id)) {
        continue;
      }
      candidates.push({ id, t: view.lastSeenAt || 0 });
    }

    candidates.sort((a, b) => a.t - b.t); // 오래된거 제거

    for (let i = 0; rowViewById.size > MAX_CACHE && i < candidates.length; i++) {
      const id = candidates[i].id;
      const view = rowViewById.get(id);
      if (!view) continue;
      view.rowEl.remove();
      rowViewById.delete(id);
    }
  };

  const resolveMetric = (row) => {
    if (typeof getMetric === "function") {
      return getMetric(row);
    }
    const dps = Number(row?.dps) || 0;
    const suffix = window.i18n?.t?.("meter.dpsSuffix", "/s") ?? "/s";
    return { value: dps, text: `${dpsFormatter.format(dps)}${suffix}` };
  };

  let lastOrderKey = "";

  // List width for fitting columns, kept by a ResizeObserver so a render
  // never has to measure. A resize that changes what fits renders again.
  let listWidth = 0;
  let lastRawRows = null;
  let lastFitKey = "";
  if (typeof ResizeObserver === "function") {
    new ResizeObserver((entries) => {
      const width = entries[entries.length - 1]?.contentRect?.width || 0;
      if (width === listWidth) return;
      listWidth = width;
      const wanted = getColumns?.() || null;
      const fitKey = wanted ? Columns.fit(wanted, listWidth).join(",") : "";
      if (fitKey !== lastFitKey && lastRawRows && !pendingRenderRows) {
        updateFromRows(lastRawRows);
      }
    }).observe(elList);
  }

  const resolveColumns = () => {
    const wanted = typeof getColumns === "function" ? getColumns() : null;
    if (!Columns || !Array.isArray(wanted) || !wanted.length) return null;
    const width = listWidth || elList.clientWidth || 0;
    return Columns.fit(wanted, width);
  };

  const widthOf = (key) => Columns.COLUMNS.find((c) => c.key === key)?.width ?? 0;

  const buildCells = (container, columns) => {
    const cells = columns.map((key, i) => {
      const cell = document.createElement("p");
      cell.className = `col col-${key}${i === 0 ? " colFirst" : ""}`;
      // The first figure is free to size to its text: it is the leftmost, so
      // its width moves nothing under the header.
      if (i > 0) cell.style.minWidth = `${widthOf(key)}px`;
      return cell;
    });
    container.replaceChildren(...cells);
    container.classList.add("hasCols");
    return cells;
  };

  // Rows put their figures in the same boxes as this header, so the labels
  // sit over the numbers at any width.
  let headerEl = null;
  let headerKey = "";
  const updateHeader = (columns, visible) => {
    if (!columns || !visible) {
      if (headerEl) headerEl.style.display = "none";
      return;
    }
    if (!headerEl) {
      headerEl = document.createElement("div");
      headerEl.className = "item colHeader";
      headerEl.setAttribute("aria-hidden", "true");
      const content = document.createElement("div");
      content.className = "content";
      const rank = document.createElement("span");
      rank.className = "rank";
      const icon = document.createElement("div");
      icon.className = "classIcon";
      const name = document.createElement("div");
      name.className = "name";
      const cols = document.createElement("div");
      cols.className = "dps";
      content.append(rank, icon, name, cols);
      headerEl.appendChild(content);
    }
    const lang = window.i18n?.getLanguage?.() || "";
    const key = `${columns.join(",")}|${lang}`;
    if (headerKey !== key) {
      const cells = buildCells(headerEl.querySelector(".dps"), columns);
      cells.forEach((cell, i) => {
        cell.textContent = getColumnLabel?.(columns[i]) ?? columns[i];
      });
      headerKey = key;
    }
    headerEl.style.display = "";
    if (elList.firstChild !== headerEl) elList.prepend(headerEl);
  };

  const renderRows = (rows, rankById) => {
    const now = nowMs();
    const nextVisibleIds = new Set();

    const hadRows = elList.classList.contains("hasRows");
    const hasRows = rows.length > 0;
    if (hadRows !== hasRows) {
      elList.classList.toggle("hasRows", hasRows);
    }

    let topMetric = 1;
    for (const row of rows) {
      const metricValue = Number(resolveMetric(row)?.value) || 0;
      if (metricValue > topMetric) topMetric = metricValue;
    }
    const visibleTotalDamage = rows.reduce((sum, row) => sum + (Number(row?.totalDamage) || 0), 0);

    // Build order key to detect if DOM reordering is needed
    let orderKey = "";
    const validRows = [];
    for (const row of rows) {
      if (!row) continue;
      const id = row.id ?? row.name;
      if (!id) continue;
      validRows.push({ row, id });
      orderKey += id + ",";
    }

    const needsReorder = orderKey !== lastOrderKey;
    lastOrderKey = orderKey;

    const columns = resolveColumns();
    const colsKey = columns ? columns.join(",") : "";
    lastFitKey = colsKey;

    for (const { row, id } of validRows) {
      nextVisibleIds.add(id);

      const view = getRowView(id);
      view.currentRow = row;
      view.lastSeenAt = now;

      if (!view.isVisible) {
        view.rowEl.style.display = "";
        view.isVisible = true;
      }

      const isUser = !!row.isUser;
      if (view.lastIsUser !== isUser) {
        view.rowEl.classList.toggle("isUser", isUser);
        view.lastIsUser = isUser;
      }

      const isIdentifying = !!row.isIdentifying;
      if (view.lastIsIdentifying !== isIdentifying) {
        view.rowEl.classList.toggle("isIdentifying", isIdentifying);
        view.lastIsIdentifying = isIdentifying;
      }

      const rowId = row.id ?? row.name ?? "";
      const nameText = row.isIdentifying
        ? window.i18n?.format?.("meter.identifyingPlayer", { id: rowId }, `#${rowId}`) ??
          `#${rowId}`
        : row.name ?? "";
      if (view.lastNameText !== nameText) {
        view.nameEl.textContent = nameText;
        view.lastNameText = nameText;
      }

      const isCjk = cjkRegex.test(nameText);
      if (view.lastIsCjk !== isCjk) {
        view.nameEl.classList.toggle("isCjk", isCjk);
        view.lastIsCjk = isCjk;
      }

      // Combat power sits beside the name, abbreviated to thousands the way
      // players quote it — 889,100 reads as "889k", 889,545 as "890k". The
      // exact figure stays on the row for the details panel. Below 500 the
      // abbreviation would collapse to "0k", so show the raw number there.
      const combatPower = Number(row.combatPower) || 0;
      const combatPowerK = Math.round(combatPower / 1000);
      const combatPowerText =
        combatPower <= 0
          ? ""
          : combatPowerK > 0
            ? `${combatPowerK.toLocaleString()}k`
            : combatPower.toLocaleString();
      if (view.lastCombatPowerText !== combatPowerText) {
        view.combatPowerEl.textContent = combatPowerText;
        view.combatPowerEl.style.display = combatPowerText ? "" : "none";
        view.lastCombatPowerText = combatPowerText;
      }

      // "Unknown" is a row whose class is not known yet: no icon, rather than
      // a broken image of a file that does not exist (issue #9).
      if (row.job && row.job !== "Unknown") {
        if (!classIconSrcByJob.has(row.job)) {
          classIconSrcByJob.set(row.job, `./assets/${row.job}.png`);
        }
        const src = classIconSrcByJob.get(row.job);
        if (view.lastClassIconSrc !== src) {
          view.classIconImg.src = src;
          view.lastClassIconSrc = src;
        }
        if (view.classIconImg.style.visibility !== "visible") {
          view.classIconImg.style.visibility = "visible";
        }
      } else {
        if (view.lastClassIconSrc) {
          view.lastClassIconSrc = "";
          view.classIconImg.removeAttribute("src");
        }
        if (view.classIconImg.style.visibility !== "hidden") {
          view.classIconImg.style.visibility = "hidden";
        }
      }

      const metric = resolveMetric(row) || { value: 0, text: "-" };
      const metricValue = Number(metric.value) || 0;
      const damageContribution =
        visibleTotalDamage > 0 ? (Number(row.totalDamage) / visibleTotalDamage) * 100 : 0;

      let contributionClass = "";
      if (damageContribution < 3) {
        contributionClass = "error";
      } else if (damageContribution < 5) {
        contributionClass = "warning";
      }
      if (view.prevContribClass !== contributionClass) {
        if (view.prevContribClass) {
          view.rowEl.classList.remove(view.prevContribClass);
        }
        if (contributionClass) {
          view.rowEl.classList.add(contributionClass);
        }
        view.prevContribClass = contributionClass;
      }

      if (view.colsKey !== colsKey) {
        if (columns) {
          view.colEls = buildCells(view.dpsContainer, columns);
        } else {
          view.dpsContainer.replaceChildren(view.dpsNumber, view.dpsContribution);
          view.dpsContainer.classList.remove("hasCols");
          view.colEls = [];
          view.lastMetricText = null;
          view.lastContributionText = null;
        }
        view.colTexts = [];
        view.colsKey = colsKey;
      }

      if (columns) {
        for (let i = 0; i < columns.length; i++) {
          const text = Columns.cellText(columns[i], row, columnFormats, damageContribution);
          if (view.colTexts[i] !== text) {
            view.colEls[i].textContent = text;
            view.colTexts[i] = text;
          }
        }
      } else {
        const metricText = metric.text;
        if (view.lastMetricText !== metricText) {
          view.dpsNumber.textContent = metricText;
          view.lastMetricText = metricText;
        }

        const contributionText = `${damageContribution.toFixed(1)}%`;
        if (view.lastContributionText !== contributionText) {
          view.dpsContribution.textContent = contributionText;
          view.lastContributionText = contributionText;
        }
      }

      const rankText = String(rankById?.get(id) ?? "");
      if (view.lastRankText !== rankText) {
        view.rankEl.textContent = rankText;
        view.rowEl.classList.toggle("isRankOne", rankText === "1");
        view.lastRankText = rankText;
      }

      const ratio = Math.max(0, Math.min(1, metricValue / topMetric));
      if (view.lastFillRatio !== ratio) {
        // Width rather than scaleX: the bar's lit leading edge is a fixed
        // 2px child, and a scaled parent would squash it (and its glow) by
        // the fill ratio. Six absolutely-positioned rows, rAF-throttled, so
        // the layout cost is not measurable.
        view.fillEl.style.width = `${(ratio * 100).toFixed(2)}%`;
        view.lastFillRatio = ratio;
      }

      // Only touch DOM order when the sorted list actually changed
      if (needsReorder) {
        elList.appendChild(view.rowEl);
      }
    }

    for (const id of lastVisibleIds) {
      if (nextVisibleIds.has(id)) continue;
      const view = rowViewById.get(id);
      if (view && view.isVisible) {
        view.rowEl.style.display = "none";
        view.isVisible = false;
      }
    }

    lastVisibleIds = nextVisibleIds;

    updateHeader(columns, validRows.length > 0);

    pruneCache(nextVisibleIds);
  };

  const flushPendingRows = () => {
    renderRowsRafId = 0;
    const rows = pendingRenderRows;
    pendingRenderRows = null;
    if (!rows) return;
    lastRawRows = rows;

    const arr = Array.isArray(rows) ? rows.slice() : [];
    const sortDirection = typeof getSortDirection === "function" ? getSortDirection() : "desc";
    arr.sort((a, b) => {
      const aMetric = Number(resolveMetric(a)?.value) || 0;
      const bMetric = Number(resolveMetric(b)?.value) || 0;
      return sortDirection === "asc" ? aMetric - bMetric : bMetric - aMetric;
    });

    // Rank is always "1 = most damage", independent of the ascending /
    // descending list toggle and of pinning the user to the top.
    const rankById = new Map();
    arr
      .slice()
      .sort((a, b) => (Number(resolveMetric(b)?.value) || 0) - (Number(resolveMetric(a)?.value) || 0))
      .forEach((row, index) => {
        const id = row?.id ?? row?.name;
        if (id) rankById.set(id, index + 1);
      });

    renderRows(getDisplayRows(arr), rankById);
  };

  const updateFromRows = (rows) => {
    pendingRenderRows = rows;
    if (renderRowsRafId) return;
    renderRowsRafId = requestAnimationFrame(flushPendingRows);
  };
  const onResetMeterUi = () => {
    if (renderRowsRafId) {
      cancelAnimationFrame(renderRowsRafId);
      renderRowsRafId = 0;
    }
    pendingRenderRows = null;
    lastRawRows = null;
    lastOrderKey = "";
    headerEl = null;
    headerKey = "";
    elList.classList.remove("hasRows");
    lastVisibleIds = new Set();

    const battleTimeEl = elList.querySelector(".battleTime");
    if (battleTimeEl) {
      elList.replaceChildren(battleTimeEl);
    } else {
      elList.replaceChildren();
    }

    rowViewById.clear();
    classIconSrcByJob.clear();
  };

  const getRowById = (id) => {
    if (id === null || id === undefined) return null;
    const view = rowViewById.get(id) || rowViewById.get(String(id));
    return view?.currentRow || null;
  };

  return { updateFromRows, onResetMeterUi, getRowById };
};
