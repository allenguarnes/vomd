//! AST types for the data-oriented chart diagrams: journey, timeline, quadrant
//! and xychart. (VOMD: sankey, radar and packet are left out.)

use std::collections::HashMap;

// ---- journey ---------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq)]
pub struct JourneyDiagram {
    pub title: Option<String>,
    pub sections: Vec<JourneySection>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JourneySection {
    pub name: String,
    pub tasks: Vec<JourneyTask>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JourneyTask {
    pub name: String,
    pub score: i32,
    pub actors: Vec<String>,
}

// ---- timeline --------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TimelineDiagram {
    pub title: Option<String>,
    pub sections: Vec<TimelineSection>,
    /// `timeline <dir>` header direction (v11.14+, e.g. `LR`/`TD`). Parsed and
    /// validated; the horizontal renderer treats it as a no-op.
    pub direction: Option<String>,
    /// `config.timeline.disableMulticolor` — when `true`, a sectionless
    /// timeline stays one flat color instead of advancing per time-period.
    pub disable_multicolor: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimelineSection {
    /// `None` for events that appear before any explicit `section` block.
    pub name: Option<String>,
    pub periods: Vec<TimelinePeriod>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimelinePeriod {
    pub label: String,
    pub events: Vec<String>,
}

// ---- quadrant --------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq)]
pub struct QuadrantDiagram {
    pub title: Option<String>,
    pub x_axis_left: Option<String>,
    pub x_axis_right: Option<String>,
    pub y_axis_bottom: Option<String>,
    pub y_axis_top: Option<String>,
    pub q1: Option<String>,
    pub q2: Option<String>,
    pub q3: Option<String>,
    pub q4: Option<String>,
    pub points: Vec<QuadrantPoint>,
    /// `classDef <name> …` style definitions, referenced by `:::name`.
    pub classes: HashMap<String, QuadrantStyle>,
    /// `config.quadrantChart.chartWidth` override for the plot square width.
    pub chart_width: Option<f64>,
    /// `config.quadrantChart.chartHeight` override for the plot square height.
    pub chart_height: Option<f64>,
    /// `config.quadrantChart.pointRadius` — default scatter-point radius.
    pub point_radius: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct QuadrantPoint {
    pub label: String,
    pub x: f64,
    pub y: f64,
    /// Third array value `[x, y, r]` or inline `radius:` — the bubble radius.
    pub radius: Option<f64>,
    pub color: Option<String>,
    pub stroke_color: Option<String>,
    pub stroke_width: Option<String>,
    /// `:::name` reference into `QuadrantDiagram::classes`.
    pub class_name: Option<String>,
}

/// Per-point styling shared by inline attributes and `classDef` definitions.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct QuadrantStyle {
    pub radius: Option<f64>,
    pub color: Option<String>,
    pub stroke_color: Option<String>,
    pub stroke_width: Option<String>,
}

// ---- xychart ---------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq)]
pub struct XyChartDiagram {
    pub horizontal: bool,
    pub title: Option<String>,
    pub x_axis: Option<XyAxis>,
    pub y_axis: Option<XyAxis>,
    pub series: Vec<XySeries>,
    /// `config.xyChart.width` — overrides the default plot width.
    pub width: Option<f64>,
    /// `config.xyChart.height` — overrides the default plot height.
    pub height: Option<f64>,
    /// `themeVariables.xyChart.plotColorPalette` — comma-separated series
    /// colors used in place of the theme's pie palette.
    pub plot_color_palette: Vec<String>,
    /// `config.xyChart.showLegend` — `None` shows the legend (upstream default).
    pub show_legend: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct XyAxis {
    pub title: Option<String>,
    pub kind: XyAxisKind,
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum XyAxisKind {
    /// Categorical labels (e.g. month names).
    Categories(Vec<String>),
    /// Numeric range `min --> max`.
    Range { min: f64, max: f64 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct XySeries {
    pub kind: XySeriesKind,
    /// Optional quoted series title (`bar "Revenue" [..]`), shown in a legend.
    pub title: Option<String>,
    pub values: Vec<f64>,
    /// Per-point labels (`line [1.5 "label", 2.3]`), aligned with `values`;
    /// each entry is `None` when the point carried no label.
    pub labels: Vec<Option<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum XySeriesKind {
    Bar,
    Line,
}
