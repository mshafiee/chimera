import { useState } from 'react'
import { Modal } from '../ui/Modal'
import { Button } from '../ui/Button'
import type { BulkRegisterRequest, BulkCleanupRequest, BulkOperationResult } from '../../api'
import { Loader2, CheckCircle, XCircle, AlertCircle } from 'lucide-react'

interface BulkOperationModalProps {
  isOpen: boolean
  onClose: () => void
  operation: 'register' | 'cleanup'
  onConfirm: (request: BulkRegisterRequest | BulkCleanupRequest) => Promise<BulkOperationResult>
}

export function BulkOperationModal({
  isOpen,
  onClose,
  operation,
  onConfirm,
}: BulkOperationModalProps) {
  const [wallets, setWallets] = useState('')
  const [forceRecreate, setForceRecreate] = useState(false)
  const [result, setResult] = useState<BulkOperationResult | null>(null)
  const [isExecuting, setIsExecuting] = useState(false)

  const operationTitle = operation === 'register' ? 'Bulk Register Webhooks' : 'Bulk Cleanup Webhooks'
  const operationDescription =
    operation === 'register'
      ? 'Register webhooks for multiple wallets. Enter wallet addresses separated by commas or newlines.'
      : 'Clean up webhooks for multiple wallets. Enter wallet addresses separated by commas or newlines.'

  const parseWallets = (input: string): string[] => {
    return input
      .split(/[,\n]+/)
      .map((w) => w.trim())
      .filter((w) => w.length > 0)
  }

  const handleConfirm = async () => {
    const walletList = parseWallets(wallets)

    if (walletList.length === 0) {
      return
    }

    setIsExecuting(true)
    setResult(null)

    try {
      const request: BulkRegisterRequest | BulkCleanupRequest =
        operation === 'register'
          ? { wallets: walletList, force_recreate: forceRecreate }
          : { wallets: walletList }

      const operationResult = await onConfirm(request)
      setResult(operationResult)
    } catch (error) {
      console.error('Bulk operation failed:', error)
    } finally {
      setIsExecuting(false)
    }
  }

  const handleClose = () => {
    if (!isExecuting) {
      setWallets('')
      setForceRecreate(false)
      setResult(null)
      onClose()
    }
  }

  return (
    <Modal isOpen={isOpen} onClose={handleClose} title={operationTitle}>
      <div className="space-y-4">
        <p className="text-sm text-text-muted">{operationDescription}</p>

        {!result && (
          <>
            <div>
              <label htmlFor="wallets" className="block text-sm font-medium mb-2">
                Wallet Addresses
              </label>
              <textarea
                id="wallets"
                value={wallets}
                onChange={(e) => setWallets(e.target.value)}
                placeholder="Enter wallet addresses separated by commas or newlines..."
                className="w-full h-32 px-3 py-2 bg-surface-light border border-border rounded-lg focus:outline-none focus:ring-2 focus:ring-shield/50 font-mono-numbers text-sm resize-none"
                disabled={isExecuting}
              />
              <p className="text-xs text-text-muted mt-1">
                {parseWallets(wallets).length} wallet{parseWallets(wallets).length !== 1 ? 's' : ''} detected
              </p>
            </div>

            {operation === 'register' && (
              <label className="flex items-center gap-2 cursor-pointer">
                <input
                  type="checkbox"
                  checked={forceRecreate}
                  onChange={(e) => setForceRecreate(e.target.checked)}
                  disabled={isExecuting}
                  className="w-4 h-4 rounded border-border bg-surface-light text-shield focus:ring-shield/50"
                />
                <span className="text-sm">Force recreate existing webhooks</span>
              </label>
            )}
          </>
        )}

        {result && (
          <div className="space-y-3">
            <div className="flex items-center justify-between p-4 bg-surface-light rounded-lg">
              <div className="flex items-center gap-3">
                {result.failed === 0 ? (
                  <CheckCircle className="w-6 h-6 text-profit" />
                ) : result.succeeded > 0 ? (
                  <AlertCircle className="w-6 h-6 text-spear" />
                ) : (
                  <XCircle className="w-6 h-6 text-loss" />
                )}
                <div>
                  <div className="font-medium">Operation Complete</div>
                  <div className="text-sm text-text-muted">
                    Processed {result.total} wallet{result.total !== 1 ? 's' : ''}
                  </div>
                </div>
              </div>
            </div>

            <div className="grid grid-cols-3 gap-4 text-center">
              <div className="p-3 bg-profit/10 rounded-lg">
                <div className="text-2xl font-bold font-mono-numbers text-profit">{result.succeeded}</div>
                <div className="text-xs text-text-muted">Succeeded</div>
              </div>
              <div className="p-3 bg-loss/10 rounded-lg">
                <div className="text-2xl font-bold font-mono-numbers text-loss">{result.failed}</div>
                <div className="text-xs text-text-muted">Failed</div>
              </div>
              <div className="p-3 bg-surface-light rounded-lg">
                <div className="text-2xl font-bold font-mono-numbers">{result.total}</div>
                <div className="text-xs text-text-muted">Total</div>
              </div>
            </div>

            {result.failed > 0 && result.results && result.results.length > 0 && (
              <div className="max-h-40 overflow-y-auto">
                <div className="text-sm font-medium mb-2">Failed Operations:</div>
                <div className="space-y-1">
                  {result.results
                    .filter((r) => !r.success)
                    .map((r, i) => (
                      <div key={i} className="text-xs font-mono-numbers text-loss flex items-start gap-2">
                        <XCircle className="w-3 h-3 mt-0.5 flex-shrink-0" />
                        <span>{r.wallet_address}: {r.error}</span>
                      </div>
                    ))}
                </div>
              </div>
            )}
          </div>
        )}

        <div className="flex justify-end gap-3 pt-2">
          <Button
            variant="ghost"
            onClick={handleClose}
            disabled={isExecuting}
          >
            {result ? 'Close' : 'Cancel'}
          </Button>
          {!result && (
            <Button
              variant={operation === 'register' ? 'primary' : 'secondary'}
              onClick={handleConfirm}
              disabled={isExecuting || parseWallets(wallets).length === 0}
            >
              {isExecuting ? (
                <>
                  <Loader2 className="w-4 h-4 mr-2 animate-spin" />
                  Processing...
                </>
              ) : (
                `Start ${operation === 'register' ? 'Registration' : 'Cleanup'}`
              )}
            </Button>
          )}
        </div>
      </div>
    </Modal>
  )
}
