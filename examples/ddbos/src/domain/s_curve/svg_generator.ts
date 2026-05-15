import { SCurve } from './entity.js';
import { SCurveStage, STAGE_COLORS, STAGE_BORDER_COLORS, STAGE_LABELS, STAGE_DESCRIPTIONS, STAGE_STRATEGIES, CurvePoint, StageBoundary, Milestone, MILESTONE_COLORS, MILESTONE_ICONS } from './value_objects.js';
import { StageDetectionService } from './services.js';
import { LocaleConfig, DEFAULT_LOCALE, stageLabel, svgLabel, milestoneLabel, t, stageStrategy } from '../../domain/shared/i18n.js';

export interface SvgOptions {
  width?: number;
  height?: number;
  showAnnotations?: boolean;
  showLegend?: boolean;
  showStageLabels?: boolean;
  locale?: LocaleConfig;
}

export class SvgCurveGenerator {
  private stageService = new StageDetectionService();

  generate(sCurve: SCurve, options: SvgOptions = {}): string {
    const locale = options.locale || DEFAULT_LOCALE;
    const lang = locale.language;
    const width = options.width || 900;
    const height = options.height || 600;
    const margin = { top: 60, right: 280, bottom: 80, left: 80 };
    const chartW = width - margin.left - margin.right;
    const chartH = height - margin.top - margin.bottom;

    const currentYear = new Date().getFullYear();

    // Determine lifecycle range from data points or use sensible defaults
    let lifecycleStart: number;
    let lifecycleEnd: number;

    if (sCurve.dataPoints.length > 0) {
      const dataMin = Math.min(...sCurve.dataPoints.map(p => p.x));
      const dataMax = Math.max(...sCurve.dataPoints.map(p => p.x));
      const span = dataMax - dataMin;
      lifecycleStart = dataMin - Math.max(2, span * 0.1);
      lifecycleEnd = Math.max(dataMax + 15, currentYear + 15);
    } else {
      lifecycleStart = currentYear - 25;
      lifecycleEnd = currentYear + 20;
    }

    const minX = lifecycleStart;
    const maxX = lifecycleEnd;
    const maxY = Math.max(sCurve.s1Parameters.L, sCurve.s2Parameters.L) * 1.15;

    const s1Points = sCurve.generateDataPoints(minX, maxX, 100);
    const s2Points = sCurve.generateS2DataPoints(minX, maxX, 100);
    const s1Boundaries = this.stageService.generateStageBoundaries(sCurve.s1Parameters, minX, maxX);

    const xScale = (x: number) => margin.left + ((x - minX) / (maxX - minX)) * chartW;
    const yScale = (y: number) => margin.top + chartH - (y / maxY) * chartH;

    const s1Path = this.buildSmoothPath(s1Points, xScale, yScale);
    const s2Path = this.buildSmoothPath(s2Points, xScale, yScale);
    const s1Line = this.buildCurveLine(s1Points, xScale, yScale);
    const s2Line = this.buildCurveLine(s2Points, xScale, yScale);

    const crossover = sCurve.getCrossoverPoint();

    let svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${width} ${height}" width="${width}" height="${height}">`;
    svg += `<defs>
      <linearGradient id="s1Grad" x1="0%" y1="0%" x2="0%" y2="100%">
        <stop offset="0%" style="stop-color:#2196F3;stop-opacity:0.3"/>
        <stop offset="100%" style="stop-color:#2196F3;stop-opacity:0.05"/>
      </linearGradient>
      <linearGradient id="s2Grad" x1="0%" y1="0%" x2="0%" y2="100%">
        <stop offset="0%" style="stop-color:#4CAF50;stop-opacity:0.3"/>
        <stop offset="100%" style="stop-color:#4CAF50;stop-opacity:0.05"/>
      </linearGradient>
      <filter id="shadow" x="-2%" y="-2%" width="104%" height="104%">
        <feDropShadow dx="1" dy="1" stdDeviation="2" flood-opacity="0.1"/>
      </filter>
    </defs>`;

    svg += `<rect x="0" y="0" width="${width}" height="${height}" fill="#fafafa" rx="8"/>`;

    svg += `<rect x="${margin.left}" y="${margin.top}" width="${chartW}" height="${chartH}" fill="white" stroke="#e0e0e0" stroke-width="1" rx="4"/>`;

    for (const boundary of s1Boundaries) {
      const x1 = xScale(boundary.startX);
      const x2 = xScale(boundary.endX);
      const color = STAGE_COLORS[boundary.stage];
      svg += `<rect x="${x1}" y="${margin.top}" width="${x2 - x1}" height="${chartH}" fill="${color}" opacity="0.4"/>`;
    }

    for (const boundary of s1Boundaries) {
      const x = xScale(boundary.startX);
      const color = STAGE_BORDER_COLORS[boundary.stage];
      svg += `<line x1="${x}" y1="${margin.top}" x2="${x}" y2="${margin.top + chartH}" stroke="${color}" stroke-width="1" stroke-dasharray="4,4" opacity="0.5"/>`;
    }

    svg += `<line x1="${margin.left}" y1="${margin.top}" x2="${margin.left}" y2="${margin.top + chartH}" stroke="#333" stroke-width="1.5"/>`;
    svg += `<line x1="${margin.left}" y1="${margin.top + chartH}" x2="${margin.left + chartW}" y2="${margin.top + chartH}" stroke="#333" stroke-width="1.5"/>`;

    for (let i = 0; i <= 5; i++) {
      const y = margin.top + (i / 5) * chartH;
      const val = maxY - (i / 5) * maxY;
      svg += `<line x1="${margin.left}" y1="${y}" x2="${margin.left + chartW}" y2="${y}" stroke="#e0e0e0" stroke-width="0.5"/>`;
      svg += `<text x="${margin.left - 8}" y="${y + 4}" text-anchor="end" font-size="10" fill="#666">${Math.round(val)}</text>`;
    }

    const yearStep = Math.ceil((maxX - minX) / 8);
    for (let year = Math.ceil(minX / yearStep) * yearStep; year <= maxX; year += yearStep) {
      const x = xScale(year);
      svg += `<line x1="${x}" y1="${margin.top}" x2="${x}" y2="${margin.top + chartH}" stroke="#e0e0e0" stroke-width="0.5"/>`;
      svg += `<text x="${x}" y="${margin.top + chartH + 20}" text-anchor="middle" font-size="10" fill="#666">${Math.round(year)}</text>`;
    }

    svg += `<path d="${s1Path}" fill="url(#s1Grad)" stroke="none"/>`;
    svg += `<path d="${s1Line}" fill="none" stroke="#2196F3" stroke-width="3" stroke-linecap="round"/>`;

    svg += `<path d="${s2Path}" fill="url(#s2Grad)" stroke="none"/>`;
    svg += `<path d="${s2Line}" fill="none" stroke="#4CAF50" stroke-width="3" stroke-linecap="round" stroke-dasharray="8,4"/>`;

    if (sCurve.dataPoints.length > 0) {
      for (const dp of sCurve.dataPoints) {
        const cx = xScale(dp.x);
        const cy = yScale(dp.y);
        svg += `<circle cx="${cx}" cy="${cy}" r="5" fill="#2196F3" stroke="white" stroke-width="2"/>`;
      }
    }

    // Render milestones
    if (sCurve.milestones.length > 0) {
      for (const m of sCurve.milestones) {
        const mx = xScale(m.year);
        const my = yScale(sCurve.getS1PerformanceAt(m.year));
        const color = MILESTONE_COLORS[m.type];
        const icon = MILESTONE_ICONS[m.type];

        // Vertical dashed line from milestone to x-axis
        svg += `<line x1="${mx}" y1="${margin.top}" x2="${mx}" y2="${margin.top + chartH}" stroke="${color}" stroke-width="1" stroke-dasharray="3,3" opacity="0.3"/>`;

        // Milestone marker (diamond shape)
        const size = 6;
        svg += `<polygon points="${mx},${my - size} ${mx + size},${my} ${mx},${my + size} ${mx - size},${my}" fill="${color}" stroke="white" stroke-width="1.5" opacity="0.9"/>`;

        // Label above the milestone
        const labelY = my - size - 8;
        const labelText = `${m.year}: ${m.label}`;
        svg += `<text x="${mx}" y="${labelY}" text-anchor="middle" font-size="9" fill="${color}" font-weight="bold">${labelText}</text>`;
      }
    }

    if (options.showAnnotations !== false) {
      const inflectionX = xScale(sCurve.s1Parameters.t0);
      const inflectionY = yScale(sCurve.s1Parameters.L / 2);
      svg += `<circle cx="${inflectionX}" cy="${inflectionY}" r="6" fill="#FF5722" stroke="white" stroke-width="2"/>`;
      svg += `<text x="${inflectionX + 10}" y="${inflectionY - 10}" font-size="10" fill="#FF5722" font-weight="bold">${svgLabel('inflectionPoint', lang)}</text>`;

      const crossoverX = xScale(crossover);
      const crossoverY = yScale(sCurve.getS1PerformanceAt(crossover));
      if (crossoverX > margin.left && crossoverX < margin.left + chartW) {
        svg += `<line x1="${crossoverX}" y1="${margin.top}" x2="${crossoverX}" y2="${margin.top + chartH}" stroke="#FF9800" stroke-width="2" stroke-dasharray="6,3"/>`;
        svg += `<text x="${crossoverX}" y="${margin.top - 10}" text-anchor="middle" font-size="10" fill="#FF9800" font-weight="bold">${svgLabel('crossover', lang)}</text>`;
      }

      const peakX = xScale(sCurve.s1Parameters.t0 + 3 / sCurve.s1Parameters.k);
      const peakY = yScale(sCurve.s1Parameters.L * 0.95);
      if (peakX > margin.left && peakX < margin.left + chartW) {
        svg += `<text x="${peakX}" y="${peakY - 15}" text-anchor="middle" font-size="10" fill="#E91E63" font-weight="bold">${svgLabel('s1Peak', lang)}</text>`;
      }
    }

    if (options.showStageLabels !== false) {
      for (const boundary of s1Boundaries) {
        const midX = xScale((boundary.startX + boundary.endX) / 2);
        const labelY = margin.top + 15;
        const color = STAGE_BORDER_COLORS[boundary.stage];
        svg += `<text x="${midX}" y="${labelY}" text-anchor="middle" font-size="11" fill="${color}" font-weight="bold">${stageLabel(boundary.stage, lang)}</text>`;
      }
    }

    svg += `<text x="${margin.left + chartW / 2}" y="${height - 15}" text-anchor="middle" font-size="13" fill="#333" font-weight="bold">${svgLabel('timeAxis', lang)}</text>`;
    svg += `<text x="15" y="${margin.top + chartH / 2}" text-anchor="middle" font-size="13" fill="#333" font-weight="bold" transform="rotate(-90, 15, ${margin.top + chartH / 2})">${sCurve.performanceMetric}</text>`;

    svg += `<text x="${margin.left + chartW / 2}" y="${margin.top - 30}" text-anchor="middle" font-size="16" fill="#333" font-weight="bold">${sCurve.technologyName} — ${svgLabel('scurveAnalysis', lang)}</text>`;
    svg += `<text x="${margin.left + chartW / 2}" y="${margin.top - 12}" text-anchor="middle" font-size="11" fill="#666">${svgLabel('currentStage', lang)}: ${stageLabel(sCurve.s1Stage, lang)} | ${svgLabel('s2Stage', lang)}: ${stageLabel(sCurve.s2Stage, lang)}</text>`;

    if (options.showLegend !== false) {
      const lx = margin.left + chartW + 15;
      let ly = margin.top + 10;

      svg += `<rect x="${lx - 5}" y="${ly - 5}" width="${margin.right - 10}" height="140" fill="white" stroke="#e0e0e0" stroke-width="1" rx="4"/>`;

      svg += `<line x1="${lx}" y1="${ly}" x2="${lx + 25}" y2="${ly}" stroke="#2196F3" stroke-width="3"/>`;
      svg += `<text x="${lx + 30}" y="${ly + 4}" font-size="11" fill="#333">${svgLabel('s1Current', lang)}</text>`;
      ly += 22;

      svg += `<line x1="${lx}" y1="${ly}" x2="${lx + 25}" y2="${ly}" stroke="#4CAF50" stroke-width="3" stroke-dasharray="8,4"/>`;
      svg += `<text x="${lx + 30}" y="${ly + 4}" font-size="11" fill="#333">${svgLabel('s2Next', lang)}</text>`;
      ly += 22;

      if (sCurve.dataPoints.length > 0) {
        svg += `<circle cx="${lx + 12}" cy="${ly - 4}" r="5" fill="#2196F3" stroke="white" stroke-width="2"/>`;
        svg += `<text x="${lx + 30}" y="${ly + 4}" font-size="11" fill="#333">${svgLabel('realData', lang)}</text>`;
        ly += 22;
      }

      svg += `<circle cx="${lx + 12}" cy="${ly - 4}" r="6" fill="#FF5722" stroke="white" stroke-width="2"/>`;
      svg += `<text x="${lx + 30}" y="${ly + 4}" font-size="11" fill="#333">${svgLabel('inflectionPoint', lang)}</text>`;
      ly += 22;

      svg += `<line x1="${lx}" y1="${ly}" x2="${lx + 25}" y2="${ly}" stroke="#FF9800" stroke-width="2" stroke-dasharray="6,3"/>`;
      svg += `<text x="${lx + 30}" y="${ly + 4}" font-size="11" fill="#333">${svgLabel('crossover', lang)}</text>`;

      ly += 35;
      svg += `<rect x="${lx - 5}" y="${ly - 5}" width="${margin.right - 10}" height="95" fill="white" stroke="#e0e0e0" stroke-width="1" rx="4"/>`;

      svg += `<text x="${lx}" y="${ly + 5}" font-size="11" fill="#333" font-weight="bold">${svgLabel('analysisSummary', lang)}</text>`;
      ly += 18;

      svg += `<text x="${lx}" y="${ly + 5}" font-size="10" fill="#666">S1: ${stageLabel(sCurve.s1Stage, lang)} (${sCurve.s1Estimated ? 'est.' : 'data'})</text>`;
      ly += 14;
      svg += `<text x="${lx}" y="${ly + 5}" font-size="10" fill="#666">S2: ${stageLabel(sCurve.s2Stage, lang)} (${t('estimated', lang)})</text>`;
      ly += 14;
      svg += `<text x="${lx}" y="${ly + 5}" font-size="10" fill="#666">${svgLabel('crossoverYear', lang)}: ~${Math.round(crossover)}</text>`;
      ly += 14;
      svg += `<text x="${lx}" y="${ly + 5}" font-size="10" fill="#666">${svgLabel('maxS1', lang)}: ${Math.round(sCurve.s1Parameters.L)} ${sCurve.performanceMetric}</text>`;
      ly += 14;
      svg += `<text x="${lx}" y="${ly + 5}" font-size="10" fill="#666">${svgLabel('maxS2', lang)}: ${Math.round(sCurve.s2Parameters.L)} ${sCurve.performanceMetric}</text>`;

      ly += 30;
      svg += `<rect x="${lx - 5}" y="${ly - 5}" width="${margin.right - 10}" height="65" fill="#fff3e0" stroke="#ff9800" stroke-width="1" rx="4"/>`;
      svg += `<text x="${lx}" y="${ly + 8}" font-size="11" fill="#e65100" font-weight="bold">${svgLabel('strategy', lang)}</text>`;
      const strategy = stageStrategy(sCurve.s1Stage, lang);
      const chars = [...strategy];
      let line = '';
      let sy = ly + 22;
      for (const char of chars) {
        if ((line + char).length > 35) {
          svg += `<text x="${lx}" y="${sy}" font-size="9" fill="#bf360c">${line.trim()}</text>`;
          line = char + ' ';
          sy += 12;
        } else {
          line += char + ' ';
        }
      }
      if (line.trim()) {
        svg += `<text x="${lx}" y="${sy}" font-size="9" fill="#bf360c">${line.trim()}</text>`;
      }

      // Milestone legend
      if (sCurve.milestones.length > 0) {
        ly += 80;
        const milestoneBoxHeight = sCurve.milestones.length * 16 + 20;
        svg += `<rect x="${lx - 5}" y="${ly - 5}" width="${margin.right - 10}" height="${milestoneBoxHeight}" fill="white" stroke="#e0e0e0" stroke-width="1" rx="4"/>`;
        svg += `<text x="${lx}" y="${ly + 5}" font-size="11" fill="#333" font-weight="bold">${svgLabel('keyEvents', lang)}</text>`;
        ly += 18;
        for (const m of sCurve.milestones) {
          const color = MILESTONE_COLORS[m.type];
          svg += `<polygon points="${lx + 6},${ly - 4} ${lx + 12},${ly + 2} ${lx + 6},${ly + 8} ${lx},${ly + 2}" fill="${color}" stroke="white" stroke-width="0.5"/>`;
          svg += `<text x="${lx + 18}" y="${ly + 4}" font-size="9" fill="#333">${m.year}: ${m.label}</text>`;
          ly += 16;
        }
      }
    }

    svg += `</svg>`;
    return svg;
  }

  generateUnicodeChart(sCurve: SCurve, width = 60, height = 20, locale?: LocaleConfig): string {
    const lang = locale?.language || DEFAULT_LOCALE.language;
    const currentYear = new Date().getFullYear();

    let minX: number;
    let maxX: number;

    if (sCurve.dataPoints.length > 0) {
      const dataMin = Math.min(...sCurve.dataPoints.map(p => p.x));
      const dataMax = Math.max(...sCurve.dataPoints.map(p => p.x));
      const span = dataMax - dataMin;
      minX = dataMin - Math.max(2, span * 0.1);
      maxX = Math.max(dataMax + 15, currentYear + 15);
    } else {
      minX = currentYear - 25;
      maxX = currentYear + 20;
    }

    const maxY = Math.max(sCurve.s1Parameters.L, sCurve.s2Parameters.L) * 1.15;

    const s1Points = sCurve.generateDataPoints(minX, maxX, width);
    const s2Points = sCurve.generateS2DataPoints(minX, maxX, width);

    const chars = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

    let chart = `\n  ${sCurve.technologyName} — ${svgLabel('scurveAnalysis', lang)} (${sCurve.performanceMetric})\n\n`;

    for (let row = height; row >= 0; row--) {
      const threshold = (row / height) * maxY;
      let line = `${Math.round(threshold).toString().padStart(4)} │`;

      for (let col = 0; col < s1Points.length; col++) {
        const s1y = s1Points[col].y;
        const s2y = s2Points[col].y;
        const s1level = Math.min(8, Math.round((s1y / maxY) * 8));
        const s2level = Math.min(8, Math.round((s2y / maxY) * 8));

        if (s1y >= threshold && s2y >= threshold) {
          line += '╳';
        } else if (s1y >= threshold) {
          line += '●';
        } else if (s2y >= threshold) {
          line += '○';
        } else {
          line += ' ';
        }
      }
      chart += line + '\n';
    }

    chart += '     └' + '─'.repeat(s1Points.length) + `→ ${t('year', lang)}\n`;
    const startLabel = Math.round(minX).toString();
    const endLabel = Math.round(maxX).toString();
    const pad = Math.max(0, s1Points.length - startLabel.length - endLabel.length);
    chart += `     ${startLabel}${' '.repeat(pad)}${endLabel}\n`;
    chart += `\n  ● = ${svgLabel('s1Current', lang)}  ○ = ${svgLabel('s2Next', lang)}  ╳ = ${svgLabel('crossover', lang)}\n`;
    chart += `  ${t('sCurveStage', lang)}: ${stageLabel(sCurve.s1Stage, lang)} → ${stageLabel(sCurve.s2Stage, lang)}\n`;

    if (sCurve.milestones.length > 0) {
      chart += `\n  ${svgLabel('keyEvents', lang)}:\n`;
      for (const m of sCurve.milestones) {
        const icon = MILESTONE_ICONS[m.type];
        chart += `    ${icon} ${m.year}: ${m.label} - ${m.description}\n`;
      }
    }

    return chart;
  }

  private buildSmoothPath(points: CurvePoint[], xScale: (x: number) => number, yScale: (y: number) => number): string {
    if (points.length < 2) return '';

    let d = `M ${xScale(points[0].x)} ${yScale(points[0].y)}`;

    for (let i = 1; i < points.length; i++) {
      const prev = points[i - 1];
      const curr = points[i];
      const cpx1 = xScale(prev.x + (curr.x - prev.x) * 0.4);
      const cpy1 = yScale(prev.y);
      const cpx2 = xScale(curr.x - (curr.x - prev.x) * 0.4);
      const cpy2 = yScale(curr.y);
      d += ` C ${cpx1} ${cpy1}, ${cpx2} ${cpy2}, ${xScale(curr.x)} ${yScale(curr.y)}`;
    }

    d += ` L ${xScale(points[points.length - 1].x)} ${yScale(0)}`;
    d += ` L ${xScale(points[0].x)} ${yScale(0)} Z`;
    return d;
  }

  private buildCurveLine(points: CurvePoint[], xScale: (x: number) => number, yScale: (y: number) => number): string {
    if (points.length < 2) return '';

    let d = `M ${xScale(points[0].x)} ${yScale(points[0].y)}`;

    for (let i = 1; i < points.length; i++) {
      const prev = points[i - 1];
      const curr = points[i];
      const cpx1 = xScale(prev.x + (curr.x - prev.x) * 0.4);
      const cpy1 = yScale(prev.y);
      const cpx2 = xScale(curr.x - (curr.x - prev.x) * 0.4);
      const cpy2 = yScale(curr.y);
      d += ` C ${cpx1} ${cpy1}, ${cpx2} ${cpy2}, ${xScale(curr.x)} ${yScale(curr.y)}`;
    }

    return d;
  }
}
