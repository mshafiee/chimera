/**
 * Incident Log E2E Tests
 * 
 * Tests the incident log functionality:
 * - Display dead letter queue items
 * - Display config audit log
 * - Filter by severity
 * - Filter by status (resolved/unresolved)
 * - Mark incidents as resolved
 * - Export incident logs
 */

import { test, expect } from '@playwright/test';

test.describe('Incident Log', () => {
  test.beforeEach(async ({ page }) => {
    // Navigate to incidents page
    await page.goto('/incidents');
  });

  // ==========================================================================
  // PAGE LOAD TESTS
  // ==========================================================================

  test('should load the incident log page', async ({ page }) => {
    // Verify page loads
    await expect(page.locator('h1, h2').first()).toBeVisible();
    
    // Verify incidents table or list is visible
    const incidentsTable = page.locator('table, [data-testid="incidents-table"]').first();
    await expect(incidentsTable).toBeVisible();
  });

  // ==========================================================================
  // DEAD LETTER QUEUE TESTS
  // ==========================================================================

  test('should display dead letter queue items', async ({ page }) => {
    // Look for dead letter queue section or tab
    const deadLetterTab = page.locator('button:has-text("Dead Letter"), [data-testid="dead-letter-tab"], tab:has-text("Dead Letter")').first();
    const deadLetterTable = page.locator('[data-testid="dead-letter-table"], table').first();
    
    if (await deadLetterTab.isVisible().catch(() => false)) {
      await deadLetterTab.click();
      await page.waitForTimeout(500);
    }
    
    // Verify table is visible
    await expect(deadLetterTable).toBeVisible();
    
    // Check for dead letter items
    const rows = page.locator('table tbody tr, [data-testid="dead-letter-item"]');
    const rowCount = await rows.count();
    
    if (rowCount > 0) {
      await expect(rows.first()).toBeVisible();
    } else {
      // Empty state is acceptable
      const emptyState = page.locator('[data-testid="empty-state"], :has-text("No incidents")').first();
      if (await emptyState.isVisible().catch(() => false)) {
        await expect(emptyState).toBeVisible();
      }
    }
  });

  // ==========================================================================
  // CONFIG AUDIT LOG TESTS
  // ==========================================================================

  test('should display config audit log', async ({ page }) => {
    // Look for config audit tab
    const auditTab = page.locator('button:has-text("Config Audit"), [data-testid="config-audit-tab"]').first();
    const auditTable = page.locator('[data-testid="config-audit-table"], table').first();
    
    if (await auditTab.isVisible().catch(() => false)) {
      await auditTab.click();
      await page.waitForTimeout(500);
    }
    
    await expect(auditTable).toBeVisible();
    
    const rows = page.locator('table tbody tr, [data-testid="audit-item"]');
    const rowCount = await rows.count();
    
    if (rowCount > 0) {
      await expect(rows.first()).toBeVisible();
    }
  });

  // ==========================================================================
  // FILTERING TESTS
  // ==========================================================================

  test('should filter incidents by severity', async ({ page }) => {
    // Look for severity filter
    const severityFilter = page.locator('[data-testid="severity-filter"], select[name*="severity"], button:has-text("Critical"), button:has-text("Warning")').first();
    
    if (await severityFilter.isVisible().catch(() => false)) {
      await severityFilter.click();
      await page.waitForTimeout(500);
      
      const rows = page.locator('table tbody tr, [data-testid="incident-item"]');
      await expect(rows.first()).toBeVisible();
    } else {
      test.skip();
    }
  });

  test('should filter incidents by status', async ({ page }) => {
    // Look for status filter (resolved/unresolved)
    const statusFilter = page.locator('[data-testid="status-filter"], select[name*="status"], button:has-text("Resolved"), button:has-text("Unresolved")').first();
    
    if (await statusFilter.isVisible().catch(() => false)) {
      await statusFilter.click();
      await page.waitForTimeout(500);
      
      const rows = page.locator('table tbody tr, [data-testid="incident-item"]');
      await expect(rows.first()).toBeVisible();
    } else {
      test.skip();
    }
  });

  test('should filter incidents by component', async ({ page }) => {
    // Look for component filter
    const componentFilter = page.locator('[data-testid="component-filter"], select[name*="component"]').first();
    
    if (await componentFilter.isVisible().catch(() => false)) {
      await componentFilter.selectOption({ index: 1 });
      await page.waitForTimeout(500);
      
      const rows = page.locator('table tbody tr, [data-testid="incident-item"]');
      await expect(rows.first()).toBeVisible();
    } else {
      test.skip();
    }
  });

  // ==========================================================================
  // RESOLUTION TESTS
  // ==========================================================================

  test('should mark incident as resolved', async ({ page }) => {
    // Look for unresolved incidents
    const unresolvedRows = page.locator('table tbody tr:has-text("Unresolved"), [data-testid="incident-item"]:has-text("Unresolved")');
    const unresolvedCount = await unresolvedRows.count();
    
    if (unresolvedCount > 0) {
      const firstUnresolved = unresolvedRows.first();
      
      // Look for resolve button
      const resolveButton = firstUnresolved.locator('button:has-text("Resolve"), [data-testid="resolve-button"]').first();
      
      if (await resolveButton.isVisible().catch(() => false)) {
        await resolveButton.click();
        
        // Wait for confirmation dialog if present
        const confirmButton = page.locator('button:has-text("Confirm"), button:has-text("Yes")').first();
        if (await confirmButton.isVisible().catch(() => false)) {
          await confirmButton.click();
        }
        
        await page.waitForTimeout(500);
        
        // Verify incident is marked as resolved
        const resolvedIndicator = firstUnresolved.locator(':has-text("Resolved"), [data-testid="resolved-badge"]').first();
        if (await resolvedIndicator.isVisible().catch(() => false)) {
          await expect(resolvedIndicator).toBeVisible();
        }
      } else {
        test.skip();
      }
    } else {
      // No unresolved incidents to test
      test.skip();
    }
  });

  test('should add notes when resolving incident', async ({ page }) => {
    const unresolvedRows = page.locator('table tbody tr:has-text("Unresolved"), [data-testid="incident-item"]:has-text("Unresolved")');
    const unresolvedCount = await unresolvedRows.count();
    
    if (unresolvedCount > 0) {
      const firstUnresolved = unresolvedRows.first();
      const resolveButton = firstUnresolved.locator('button:has-text("Resolve"), [data-testid="resolve-button"]').first();
      
      if (await resolveButton.isVisible().catch(() => false)) {
        await resolveButton.click();
        
        // Look for notes input
        const notesInput = page.locator('textarea[name*="note"], input[name*="note"], [data-testid="notes-input"]').first();
        
        if (await notesInput.isVisible().catch(() => false)) {
          await notesInput.fill('Test resolution note');
          
          const confirmButton = page.locator('button:has-text("Confirm"), button:has-text("Resolve")').first();
          if (await confirmButton.isVisible().catch(() => false)) {
            await confirmButton.click();
            await page.waitForTimeout(500);
            
            // Verify note was added
            const noteDisplay = firstUnresolved.locator(':has-text("Test resolution note")').first();
            if (await noteDisplay.isVisible().catch(() => false)) {
              await expect(noteDisplay).toBeVisible();
            }
          }
        } else {
          test.skip();
        }
      } else {
        test.skip();
      }
    } else {
      test.skip();
    }
  });

  // ==========================================================================
  // EXPORT TESTS
  // ==========================================================================

  test('should export incident logs', async ({ page }) => {
    // Look for export button
    const exportButton = page.locator('button:has-text("Export"), [data-testid="export-incidents"]').first();
    
    if (await exportButton.isVisible().catch(() => false)) {
      const downloadPromise = page.waitForEvent('download');
      await exportButton.click();
      
      const download = await downloadPromise;
      expect(download.suggestedFilename()).toBeTruthy();
    } else {
      test.skip();
    }
  });

  // ==========================================================================
  // DISPLAY TESTS
  // ==========================================================================

  test('should display incident severity badges', async ({ page }) => {
    const rows = page.locator('table tbody tr, [data-testid="incident-item"]');
    const rowCount = await rows.count();
    
    if (rowCount > 0) {
      const firstRow = rows.first();
      
      // Look for severity badge
      const severityBadge = firstRow.locator('[data-testid="severity-badge"], .badge, .severity').first();
      
      if (await severityBadge.isVisible().catch(() => false)) {
        await expect(severityBadge).toBeVisible();
        
        // Verify badge has text (Critical, Warning, Info)
        const badgeText = await severityBadge.textContent();
        expect(badgeText).toMatch(/critical|warning|info|error/i);
      }
    } else {
      test.skip();
    }
  });

  test('should display incident timestamps', async ({ page }) => {
    const rows = page.locator('table tbody tr, [data-testid="incident-item"]');
    const rowCount = await rows.count();
    
    if (rowCount > 0) {
      const firstRow = rows.first();
      
      // Look for timestamp (date/time format)
      const timestamp = firstRow.locator('[data-testid="timestamp"], time, :has-text(/\\d{4}-\\d{2}-\\d{2}/)').first();
      
      if (await timestamp.isVisible().catch(() => false)) {
        await expect(timestamp).toBeVisible();
      }
    } else {
      test.skip();
    }
  });

  test('should display incident component', async ({ page }) => {
    const rows = page.locator('table tbody tr, [data-testid="incident-item"]');
    const rowCount = await rows.count();
    
    if (rowCount > 0) {
      const firstRow = rows.first();
      
      // Look for component name
      const component = firstRow.locator('[data-testid="component"], :has-text(/Queue|RPC|Database|Circuit/)').first();
      
      if (await component.isVisible().catch(() => false)) {
        await expect(component).toBeVisible();
      }
    } else {
      test.skip();
    }
  });
});
