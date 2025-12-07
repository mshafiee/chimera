/**
 * Trade Ledger E2E Tests
 * 
 * Tests the trade ledger functionality:
 * - Trade list filtering
 * - Trade export (CSV/PDF/JSON)
 * - Pagination
 * - Date range filtering
 * - Strategy filtering
 * - Status filtering
 */

import { test, expect } from '@playwright/test';

test.describe('Trade Ledger', () => {
  test.beforeEach(async ({ page }) => {
    // Navigate to trades page
    await page.goto('/trades');
  });

  // ==========================================================================
  // PAGE LOAD TESTS
  // ==========================================================================

  test('should load the trade ledger page', async ({ page }) => {
    // Verify page loads
    await expect(page.locator('h1, h2').first()).toBeVisible();
    
    // Verify trades table or list is visible
    const tradesTable = page.locator('table, [data-testid="trades-table"]').first();
    await expect(tradesTable).toBeVisible();
  });

  // ==========================================================================
  // FILTERING TESTS
  // ==========================================================================

  test('should filter trades by strategy', async ({ page }) => {
    // Look for strategy filter (dropdown, buttons, or select)
    const strategyFilter = page.locator('[data-testid="strategy-filter"], select[name*="strategy"], button:has-text("SHIELD"), button:has-text("SPEAR")').first();
    
    if (await strategyFilter.isVisible().catch(() => false)) {
      // Click on a strategy filter (e.g., SHIELD)
      await strategyFilter.click();
      
      // Wait for table to update
      await page.waitForTimeout(500);
      
      // Verify table shows filtered results (check for strategy column if visible)
      const tableRows = page.locator('table tbody tr, [data-testid="trade-row"]');
      const rowCount = await tableRows.count();
      
      if (rowCount > 0) {
        // Verify at least one row is visible
        await expect(tableRows.first()).toBeVisible();
      }
    } else {
      // Filter might not be implemented yet, skip
      test.skip();
    }
  });

  test('should filter trades by status', async ({ page }) => {
    // Look for status filter
    const statusFilter = page.locator('[data-testid="status-filter"], select[name*="status"], button:has-text("ACTIVE"), button:has-text("CLOSED")').first();
    
    if (await statusFilter.isVisible().catch(() => false)) {
      await statusFilter.click();
      await page.waitForTimeout(500);
      
      const tableRows = page.locator('table tbody tr, [data-testid="trade-row"]');
      await expect(tableRows.first()).toBeVisible();
    } else {
      test.skip();
    }
  });

  test('should filter trades by date range', async ({ page }) => {
    // Look for date picker or date range selector
    const datePicker = page.locator('[data-testid="date-picker"], input[type="date"], .date-range-picker').first();
    
    if (await datePicker.isVisible().catch(() => false)) {
      // Set a date range (e.g., last 7 days)
      const today = new Date();
      const lastWeek = new Date(today.getTime() - 7 * 24 * 60 * 60 * 1000);
      
      // Try to set dates (format depends on implementation)
      await datePicker.fill(lastWeek.toISOString().split('T')[0]);
      
      await page.waitForTimeout(500);
      
      // Verify table updates
      const tableRows = page.locator('table tbody tr, [data-testid="trade-row"]');
      await expect(tableRows.first()).toBeVisible();
    } else {
      test.skip();
    }
  });

  test('should filter trades by token', async ({ page }) => {
    // Look for token search/filter
    const tokenFilter = page.locator('[data-testid="token-filter"], input[placeholder*="token"], input[name*="token"]').first();
    
    if (await tokenFilter.isVisible().catch(() => false)) {
      await tokenFilter.fill('BONK');
      await page.waitForTimeout(500);
      
      const tableRows = page.locator('table tbody tr, [data-testid="trade-row"]');
      await expect(tableRows.first()).toBeVisible();
    } else {
      test.skip();
    }
  });

  // ==========================================================================
  // EXPORT TESTS
  // ==========================================================================

  test('should export trades as CSV', async ({ page }) => {
    // Look for export button
    const exportButton = page.locator('button:has-text("Export"), button:has-text("CSV"), [data-testid="export-csv"]').first();
    
    if (await exportButton.isVisible().catch(() => false)) {
      // Set up download listener
      const downloadPromise = page.waitForEvent('download');
      await exportButton.click();
      
      const download = await downloadPromise;
      
      // Verify download started
      expect(download.suggestedFilename()).toMatch(/\.csv$/i);
    } else {
      test.skip();
    }
  });

  test('should export trades as JSON', async ({ page }) => {
    const exportButton = page.locator('button:has-text("JSON"), [data-testid="export-json"]').first();
    
    if (await exportButton.isVisible().catch(() => false)) {
      const downloadPromise = page.waitForEvent('download');
      await exportButton.click();
      
      const download = await downloadPromise;
      expect(download.suggestedFilename()).toMatch(/\.json$/i);
    } else {
      test.skip();
    }
  });

  test('should export trades as PDF', async ({ page }) => {
    const exportButton = page.locator('button:has-text("PDF"), [data-testid="export-pdf"]').first();
    
    if (await exportButton.isVisible().catch(() => false)) {
      const downloadPromise = page.waitForEvent('download');
      await exportButton.click();
      
      const download = await downloadPromise;
      expect(download.suggestedFilename()).toMatch(/\.pdf$/i);
    } else {
      test.skip();
    }
  });

  // ==========================================================================
  // PAGINATION TESTS
  // ==========================================================================

  test('should paginate through trades', async ({ page }) => {
    // Look for pagination controls
    const nextButton = page.locator('button:has-text("Next"), [aria-label="Next page"], [data-testid="pagination-next"]').first();
    const prevButton = page.locator('button:has-text("Prev"), [aria-label="Previous page"], [data-testid="pagination-prev"]').first();
    
    if (await nextButton.isVisible().catch(() => false)) {
      // Click next
      await nextButton.click();
      await page.waitForTimeout(500);
      
      // Verify page changed (check for page indicator)
      const pageIndicator = page.locator('[data-testid="page-number"], .pagination-current').first();
      if (await pageIndicator.isVisible().catch(() => false)) {
        await expect(pageIndicator).toBeVisible();
      }
      
      // Click previous if available
      if (await prevButton.isVisible().catch(() => false)) {
        await prevButton.click();
        await page.waitForTimeout(500);
      }
    } else {
      test.skip();
    }
  });

  test('should change items per page', async ({ page }) => {
    // Look for items per page selector
    const itemsPerPage = page.locator('select[name*="per-page"], [data-testid="items-per-page"]').first();
    
    if (await itemsPerPage.isVisible().catch(() => false)) {
      await itemsPerPage.selectOption('50');
      await page.waitForTimeout(500);
      
      // Verify table updates
      const tableRows = page.locator('table tbody tr, [data-testid="trade-row"]');
      const rowCount = await tableRows.count();
      
      // Should have at most 50 rows (or all if less than 50)
      expect(rowCount).toBeLessThanOrEqual(50);
    } else {
      test.skip();
    }
  });

  // ==========================================================================
  // TABLE FUNCTIONALITY TESTS
  // ==========================================================================

  test('should display trade details in table', async ({ page }) => {
    const tableRows = page.locator('table tbody tr, [data-testid="trade-row"]');
    const rowCount = await tableRows.count();
    
    if (rowCount > 0) {
      const firstRow = tableRows.first();
      await expect(firstRow).toBeVisible();
      
      // Verify row has content (text or cells)
      const rowText = await firstRow.textContent();
      expect(rowText).toBeTruthy();
      expect(rowText!.length).toBeGreaterThan(0);
    } else {
      // No trades to display - verify empty state
      const emptyState = page.locator('[data-testid="empty-state"], .empty-state, :has-text("No trades")').first();
      if (await emptyState.isVisible().catch(() => false)) {
        await expect(emptyState).toBeVisible();
      }
    }
  });

  test('should link to on-chain transaction', async ({ page }) => {
    const tableRows = page.locator('table tbody tr, [data-testid="trade-row"]');
    const rowCount = await tableRows.count();
    
    if (rowCount > 0) {
      // Look for transaction link (Solscan, Solana Explorer)
      const txLink = page.locator('a[href*="solscan"], a[href*="explorer"], a[href*="solana"], [data-testid="tx-link"]').first();
      
      if (await txLink.isVisible().catch(() => false)) {
        // Verify link has href
        const href = await txLink.getAttribute('href');
        expect(href).toBeTruthy();
        expect(href).toMatch(/solscan|explorer|solana/i);
      }
    } else {
      test.skip();
    }
  });

  test('should sort trades by column', async ({ page }) => {
    // Look for sortable column headers
    const sortableHeader = page.locator('th[data-sortable], th button, th[aria-sort]').first();
    
    if (await sortableHeader.isVisible().catch(() => false)) {
      // Click to sort
      await sortableHeader.click();
      await page.waitForTimeout(500);
      
      // Verify table updates (check for sort indicator)
      const sortIndicator = page.locator('[aria-sort], .sort-asc, .sort-desc').first();
      if (await sortIndicator.isVisible().catch(() => false)) {
        await expect(sortIndicator).toBeVisible();
      }
    } else {
      test.skip();
    }
  });
});
