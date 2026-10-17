(() => {
  "use strict";

  const status = document.querySelector("#status");
  const latestWeight = document.querySelector("#latest-weight");
  const latestWeightPounds = document.querySelector("#latest-weight-lbs");
  const latestDate = document.querySelector("#latest-date");
  const weightChange = document.querySelector("#weight-change");
  const weightChangePounds = document.querySelector("#weight-change-lbs");
  const measurementCount = document.querySelector("#measurement-count");
  const chartElement = document.querySelector("#chart");
  const emptyState = document.querySelector("#empty-state");
  const rangeButtons = [...document.querySelectorAll("[data-months]")];
  const dateTime = new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  });

  let chart;
  let resizeObserver;
  let latestTimestamp;

  function kilograms(hundredths) {
    return hundredths / 100;
  }

  function pounds(kilogramsValue) {
    return kilogramsValue * 2.2046226218;
  }

  function updateSummary(reports) {
    measurementCount.textContent = reports.length.toLocaleString();

    if (reports.length === 0) {
      latestWeight.textContent = "—";
      latestWeightPounds.textContent = "";
      latestDate.textContent = "No measurements yet";
      weightChange.textContent = "—";
      weightChangePounds.textContent = "";
      return;
    }

    const first = reports[0];
    const latest = reports.at(-1);
    const change = kilograms(latest.weight - first.weight);

    latestWeight.textContent = `${kilograms(latest.weight).toFixed(2)} kg`;
    latestWeightPounds.textContent = `(${pounds(kilograms(latest.weight)).toFixed(1)} lbs)`;
    latestDate.textContent = dateTime.format(new Date(latest.timestamp * 1000));
    weightChange.textContent = `${change > 0 ? "+" : ""}${change.toFixed(2)} kg`;
    const poundsChange = pounds(change);
    weightChangePounds.textContent = `(${poundsChange > 0 ? "+" : ""}${poundsChange.toFixed(1)} lbs)`;
    weightChange.classList.toggle("positive", change > 0);
    weightChange.classList.toggle("negative", change < 0);
  }

  function paddedWeightRange(_u, minimum, maximum) {
    const span = Math.max(maximum - minimum, 1);
    const padding = Math.max(span * 0.15, 0.25);
    return [minimum - padding, maximum + padding];
  }

  function subtractMonths(timestamp, months) {
    const date = new Date(timestamp * 1000);
    const day = date.getUTCDate();
    date.setUTCDate(1);
    date.setUTCMonth(date.getUTCMonth() - months);
    const lastDay = new Date(Date.UTC(
      date.getUTCFullYear(),
      date.getUTCMonth() + 1,
      0,
    )).getUTCDate();
    date.setUTCDate(Math.min(day, lastDay));
    return date.getTime() / 1000;
  }

  function selectRange(selectedButton) {
    if (!chart || latestTimestamp == null) {
      return;
    }

    const months = Number(selectedButton.dataset.months);
    chart.setScale("x", {
      min: subtractMonths(latestTimestamp, months),
      max: latestTimestamp,
    });
    for (const button of rangeButtons) {
      button.setAttribute("aria-pressed", String(button === selectedButton));
    }
  }

  function drawChart(reports) {
    if (reports.length === 0) {
      emptyState.hidden = false;
      chartElement.hidden = true;
      return;
    }

    emptyState.hidden = true;
    chartElement.hidden = false;

    const data = [
      reports.map((report) => report.timestamp),
      reports.map((report) => kilograms(report.weight)),
    ];
    latestTimestamp = reports.at(-1).timestamp;
    const options = {
      width: Math.max(chartElement.clientWidth, 280),
      height: window.innerWidth <= 720 ? 300 : 340,
      legend: { show: false },
      cursor: {
        drag: { x: true, y: false },
      },
      scales: {
        x: { time: true },
        y: { range: paddedWeightRange },
      },
      axes: [
        {
          stroke: "#737d87",
          grid: { stroke: "#21262d", width: 1 },
          ticks: { stroke: "#30363d", width: 1 },
          font: "12px system-ui, sans-serif",
        },
        {
          scale: "y",
          stroke: "#737d87",
          grid: { stroke: "#21262d", width: 1 },
          ticks: { stroke: "#30363d", width: 1 },
          font: "12px system-ui, sans-serif",
          values: (_u, values) => values.map((value) => `${value.toFixed(1)} kg`),
          size: 72,
        },
        {
          scale: "y",
          side: 1,
          stroke: "#626b74",
          grid: { show: false },
          ticks: { stroke: "#30363d", width: 1 },
          font: "11px system-ui, sans-serif",
          values: (_u, values) => values.map((value) => `${pounds(value).toFixed(1)} lbs`),
          size: 66,
        },
      ],
      series: [
        { label: "Time" },
        {
          label: "Weight",
          stroke: "#8ca3b8",
          width: 1.5,
          points: {
            show: true,
            size: 4,
            width: 1.5,
            stroke: "#8ca3b8",
            fill: "#11161c",
          },
          value: (_u, value) => value == null
            ? "—"
            : `${value.toFixed(2)} kg (${pounds(value).toFixed(1)} lbs)`,
        },
      ],
    };

    chart = new uPlot(options, data, chartElement);
    resizeObserver = new ResizeObserver(([entry]) => {
      const width = Math.floor(entry.contentRect.width);
      const height = window.innerWidth <= 720 ? 300 : 340;
      if (width > 0 && (chart.width !== width || chart.height !== height)) {
        chart.setSize({ width, height });
      }
    });
    resizeObserver.observe(chartElement);
    for (const button of rangeButtons) {
      button.disabled = false;
    }
  }

  async function loadReports() {
    try {
      const response = await fetch("/api/reports", {
        headers: { accept: "application/json" },
      });
      if (!response.ok) {
        throw new Error(`server returned HTTP ${response.status}`);
      }

      const reports = await response.json();
      reports.sort((left, right) => left.timestamp - right.timestamp);
      updateSummary(reports);
      drawChart(reports);
      status.textContent = reports.length === 0 ? "Waiting for data" : "Up to date";
    } catch (error) {
      console.error("Could not load reports", error);
      status.textContent = "Could not load data";
      status.classList.add("error");
      emptyState.hidden = false;
      emptyState.textContent = "The measurements could not be loaded. Try refreshing the page.";
      chartElement.hidden = true;
    }
  }

  window.addEventListener("pagehide", () => {
    resizeObserver?.disconnect();
    chart?.destroy();
  });

  for (const button of rangeButtons) {
    button.addEventListener("click", () => selectRange(button));
  }

  loadReports();
})();
