import { HTMLAttributes, TdHTMLAttributes, ThHTMLAttributes, forwardRef } from 'react'
import { clsx } from 'clsx'

interface TableProps extends HTMLAttributes<HTMLTableElement> {}

export const Table = forwardRef<HTMLTableElement, TableProps>(
  ({ className, children, ...props }, ref) => {
    return (
      <div className="overflow-x-auto">
        <table
          ref={ref}
          className={clsx('w-full text-sm', className)}
          {...props}
        >
          {children}
        </table>
      </div>
    )
  }
)

Table.displayName = 'Table'

interface TableHeaderProps extends HTMLAttributes<HTMLTableSectionElement> {}

export const TableHeader = forwardRef<HTMLTableSectionElement, TableHeaderProps>(
  ({ className, children, ...props }, ref) => {
    return (
      <thead
        ref={ref}
        className={clsx('border-b border-border', className)}
        {...props}
      >
        {children}
      </thead>
    )
  }
)

TableHeader.displayName = 'TableHeader'

interface TableBodyProps extends HTMLAttributes<HTMLTableSectionElement> {}

export const TableBody = forwardRef<HTMLTableSectionElement, TableBodyProps>(
  ({ className, children, ...props }, ref) => {
    return (
      <tbody ref={ref} className={clsx('divide-y divide-border', className)} {...props}>
        {children}
      </tbody>
    )
  }
)

TableBody.displayName = 'TableBody'

interface TableRowProps extends HTMLAttributes<HTMLTableRowElement> {
  hoverable?: boolean
}

export const TableRow = forwardRef<HTMLTableRowElement, TableRowProps>(
  ({ className, hoverable = true, children, ...props }, ref) => {
    return (
      <tr
        ref={ref}
        className={clsx(
          'transition-colors',
          hoverable && 'hover:bg-surface-light cursor-pointer',
          className
        )}
        {...props}
      >
        {children}
      </tr>
    )
  }
)

TableRow.displayName = 'TableRow'

interface TableHeadProps extends ThHTMLAttributes<HTMLTableCellElement> {
  sortable?: boolean
  sorted?: 'asc' | 'desc' | false
}

export const TableHead = forwardRef<HTMLTableCellElement, TableHeadProps>(
  ({ className, sortable, sorted, children, ...props }, ref) => {
    return (
      <th
        ref={ref}
        className={clsx(
          'px-4 py-3 text-left text-xs font-semibold uppercase tracking-wider text-text-muted',
          sortable && 'cursor-pointer hover:text-text',
          className
        )}
        {...props}
      >
        <div className="flex items-center gap-1">
          {children}
          {sortable && (
            <span className="text-text-muted">
              {sorted === 'asc' && '↑'}
              {sorted === 'desc' && '↓'}
              {!sorted && '↕'}
            </span>
          )}
        </div>
      </th>
    )
  }
)

TableHead.displayName = 'TableHead'

interface TableCellProps extends TdHTMLAttributes<HTMLTableCellElement> {
  mono?: boolean
}

export const TableCell = forwardRef<HTMLTableCellElement, TableCellProps>(
  ({ className, mono, children, ...props }, ref) => {
    return (
      <td
        ref={ref}
        className={clsx(
          'px-4 py-3 whitespace-nowrap',
          mono && 'font-mono-numbers',
          className
        )}
        {...props}
      >
        {children}
      </td>
    )
  }
)

TableCell.displayName = 'TableCell'
