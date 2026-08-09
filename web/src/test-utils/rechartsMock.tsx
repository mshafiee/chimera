import React from 'react'

// Recharts stub used by tests: renders children so the chart components'
// mapping/derivation code runs, and invokes tooltip/label render props so
// their closures are covered too.

const SAMPLE_PAYLOAD = {
  name: 'Sample',
  value: 12.34,
  percentage: 12.3,
  count: 5,
  amount: 1.5,
  full: 'Sample Token Name',
  range: '0-20',
  score: 0.75,
  time: '1:00 PM',
  wallets: 3,
  total: 5,
  symbol: 'SAMPLE',
  consensus_percent: '80',
  quality_score: '75',
  divergence_score: '55',
  volatility: 1.23,
  timestamp: '2025-01-01T00:00:00Z',
  average_score: 0.6,
}

function runRenderProps(props: Record<string, unknown>) {
  if (typeof props.formatter === 'function') {
    (props.formatter as (v: number, n: string) => unknown)(12.34, 'nav')
  }
  if (typeof props.labelFormatter === 'function') {
    (props.labelFormatter as (l: unknown) => unknown)('2025-01-01T00:00:00Z')
  }
  if (typeof props.tickFormatter === 'function') {
    (props.tickFormatter as (v: unknown) => unknown)('2025-01-01T00:00:00Z')
  }
  if (typeof props.label === 'function') {
    (props.label as (e: unknown) => unknown)(SAMPLE_PAYLOAD)
  }
}

export function ResponsiveContainer(props: React.PropsWithChildren<Record<string, unknown>>) {
  return <div data-testid="responsive-container">{props.children}</div>
}

export function Tooltip(props: Record<string, unknown>) {
  runRenderProps(props)
  const content = props.content
  if (content) {
    const payload = [{ ...SAMPLE_PAYLOAD, payload: SAMPLE_PAYLOAD }]
    const activeProps = { active: true, payload, label: '2025-01-01T00:00:00Z' }
    // render once with data (active branch) and once without (inactive branch)
    if (typeof content === 'function') {
      const Content = content as (p: Record<string, unknown>) => React.ReactNode
      return (
        <>
          {Content(activeProps)}
          {Content({})}
        </>
      )
    }
    if (React.isValidElement(content)) {
      return (
        <>
          {React.cloneElement(content, activeProps as Record<string, unknown>)}
          {React.cloneElement(content, {} as Record<string, unknown>)}
        </>
      )
    }
  }
  return null
}

function Chart(props: React.PropsWithChildren<Record<string, unknown>>) {
  runRenderProps(props)
  return <div>{props.children}</div>
}

export const AreaChart = Chart
export const BarChart = Chart
export const LineChart = Chart
export const PieChart = Chart
export const ScatterChart = Chart

export function Cell() {
  return null
}

export function XAxis(props: Record<string, unknown>) {
  runRenderProps(props)
  return null
}

export function YAxis(props: Record<string, unknown>) {
  runRenderProps(props)
  return null
}

export function ZAxis() {
  return null
}

export function CartesianGrid() {
  return null
}

export function Legend() {
  return null
}

export function ReferenceLine() {
  return null
}

export function Area() {
  return null
}

export function Bar(props: React.PropsWithChildren<Record<string, unknown>>) {
  return <div>{props.children}</div>
}

export function Line() {
  return null
}

export function Scatter() {
  return null
}

export function Pie(props: React.PropsWithChildren<Record<string, unknown>>) {
  runRenderProps(props)
  return <div>{props.children}</div>
}
