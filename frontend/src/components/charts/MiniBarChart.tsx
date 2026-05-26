import type { ReactNode } from 'react'
import { Bar, BarChart, CartesianGrid, ResponsiveContainer, Tooltip, XAxis, YAxis } from 'recharts'

export type MiniBarTooltipPayload = Array<{
  value: number | string
  dataKey?: string
  payload: Record<string, unknown>
}>

export interface MiniBarTooltipProps {
  active?: boolean
  payload?: MiniBarTooltipPayload
  label?: string | number
}

function DefaultMiniTooltip({ active, payload }: MiniBarTooltipProps) {
  if (!active || !payload || payload.length === 0) return null
  const { value } = payload[0]
  return (
    <div className="rounded-md border border-border bg-popover px-2 py-1 text-xs shadow-md">
      <div className="font-semibold tabular-nums text-foreground">{value}</div>
    </div>
  )
}

export function MiniBarChart({ data, dataKey = 'count', color, height = 120, tooltipContent }: {
  data: Array<Record<string, unknown>>
  dataKey?: string
  color: string
  height?: number
  /** Override the popup body. Receives recharts tooltip props (active/payload/label). */
  tooltipContent?: (props: MiniBarTooltipProps) => ReactNode
}) {
  const content = tooltipContent ?? ((props: MiniBarTooltipProps) => <DefaultMiniTooltip {...props} />)
  return (
    <ResponsiveContainer width="100%" height={height}>
      <BarChart data={data} margin={{ top: 2, right: 2, left: -28, bottom: 0 }}>
        <CartesianGrid strokeDasharray="3 3" className="stroke-border" vertical={false} />
        <XAxis dataKey="date" tick={{ fontSize: 9 }} interval="preserveStartEnd" />
        <YAxis tick={{ fontSize: 9 }} allowDecimals={false} />
        <Tooltip cursor={{ fill: 'hsl(var(--muted) / 0.4)' }} content={content as never} />
        <Bar dataKey={dataKey} fill={color} radius={[3, 3, 0, 0]} />
      </BarChart>
    </ResponsiveContainer>
  )
}
