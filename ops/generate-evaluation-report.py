#!/usr/bin/env python3
"""
generate-evaluation-report.py - Comprehensive evaluation report generator

Generates detailed evaluation reports after the 10-day evaluation period,
covering all aspects of system performance, costs, risks, and recommendations.

Usage:
    python3 generate-evaluation-report.py \
        --evaluation-dir /opt/chimera/evaluation \
        --database /opt/chimera/evaluation/evaluation.db \
        --output /opt/chimera/evaluation/FINAL_EVALUATION_REPORT.html
"""

import argparse
import json
import sqlite3
import sys
from datetime import datetime, timedelta
from pathlib import Path
from typing import Dict, Any, List, Optional
from dataclasses import dataclass
import os


@dataclass
class EvaluationReportData:
    """Container for comprehensive evaluation report data."""
    executive_summary: Dict[str, Any]
    performance_analysis: Dict[str, Any]
    cost_analysis: Dict[str, Any]
    risk_analysis: Dict[str, Any]
    system_health: Dict[str, Any]
    code_profiling: Dict[str, Any]
    database_performance: Dict[str, Any]
    network_analysis: Dict[str, Any]
    anomalies_detected: List[Dict[str, Any]]
    recommendations: List[Dict[str, Any]]
    appendices: Dict[str, Any]


class EvaluationReportGenerator:
    """Generate comprehensive evaluation reports."""

    def __init__(self, db_path: str, eval_dir: Path):
        """Initialize the report generator.

        Args:
            db_path: Path to evaluation database
            eval_dir: Path to evaluation directory with raw data
        """
        self.db_path = Path(db_path)
        self.eval_dir = Path(eval_dir)
        self.conn = None

    def connect(self):
        """Connect to evaluation database."""
        try:
            self.conn = sqlite3.connect(str(self.db_path))
            self.conn.row_factory = sqlite3.Row
            print(f"Connected to evaluation database: {self.db_path}")
        except sqlite3.Error as e:
            print(f"Failed to connect to database: {e}")
            sys.exit(1)

    def close(self):
        """Close database connection."""
        if self.conn:
            self.conn.close()

    def generate_executive_summary(self) -> Dict[str, Any]:
        """Generate executive summary of the evaluation.

        Returns:
            Dictionary with executive summary data
        """
        cursor = self.conn.cursor()

        # Get overall evaluation statistics
        cursor.execute('''
            SELECT
                COUNT(DISTINCT day_number) as days_evaluated,
                SUM(total_trades_today) as total_trades,
                SUM(successful_trades_today) as total_successful,
                AVG(avg_trade_latency_ms) as avg_latency,
                AVG(total_pnl_sol) as total_pnl,
                SUM(total_costs_sol) as total_costs,
                MAX(max_drawdown_percent) as max_drawdown
            FROM evaluation_snapshots
        ''')

        stats = cursor.fetchone()

        # Calculate success rate
        total_trades = stats['total_trades'] or 0
        total_successful = stats['total_successful'] or 0
        success_rate = (total_successful / total_trades * 100) if total_trades > 0 else 0

        # Get anomaly statistics
        cursor.execute('''
            SELECT
                COUNT(*) as total_anomalies,
                SUM(CASE WHEN severity = 'CRITICAL' THEN 1 ELSE 0 END) as critical_anomalies
            FROM evaluation_anomalies
            WHERE resolved = 0
        ''')

        anomaly_stats = cursor.fetchone()

        return {
            'evaluation_period': {
                'days_evaluated': stats['days_evaluated'] or 0,
                'start_date': self._get_evaluation_start_date(),
                'end_date': self._get_evaluation_end_date(),
                'evaluation_duration_days': stats['days_evaluated'] or 0
            },
            'trading_performance': {
                'total_trades': total_trades,
                'successful_trades': total_successful,
                'failed_trades': total_trades - total_successful,
                'success_rate_percent': success_rate,
                'total_pnl_sol': stats['total_pnl'] or 0.0,
                'total_costs_sol': stats['total_costs'] or 0.0,
                'net_pnl_sol': (stats['total_pnl'] or 0.0) - (stats['total_costs'] or 0.0)
            },
            'system_performance': {
                'avg_trade_latency_ms': stats['avg_latency'] or 0.0,
                'max_drawdown_percent': stats['max_drawdown'] or 0.0,
                'total_anomalies': anomaly_stats['total_anomalies'] or 0,
                'critical_anomalies': anomaly_stats['critical_anomalies'] or 0
            },
            'overall_grade': self._calculate_overall_grade(success_rate, stats['avg_latency'] or 0, anomaly_stats['critical_anomalies'] or 0)
        }

    def generate_performance_analysis(self) -> Dict[str, Any]:
        """Generate detailed performance analysis.

        Returns:
            Dictionary with performance analysis data
        """
        cursor = self.conn.cursor()

        # Hourly performance patterns
        cursor.execute('''
            SELECT
                hour_number,
                AVG(avg_trade_latency_ms) as avg_latency,
                AVG(p95_trade_latency_ms) as avg_p95_latency,
                AVG(p99_trade_latency_ms) as avg_p99_latency,
                SUM(total_trades_today) as hourly_trades,
                AVG(queue_depth) as avg_queue_depth
            FROM evaluation_snapshots
            GROUP BY hour_number
            ORDER BY hour_number
        ''')

        hourly_patterns = [dict(row) for row in cursor.fetchall()]

        # Daily performance trends
        cursor.execute('''
            SELECT
                day_number,
                AVG(avg_trade_latency_ms) as day_avg_latency,
                SUM(total_trades_today) as day_trades,
                AVG(total_pnl_sol) as day_pnl,
                AVG(rpc_latency_avg_ms) as day_rpc_latency
            FROM evaluation_snapshots
            GROUP BY day_number
            ORDER BY day_number
        ''')

        daily_trends = [dict(row) for row in cursor.fetchall()]

        # Performance degradation analysis
        day1_performance = daily_trends[0] if daily_trends else {}
        day10_performance = daily_trends[-1] if len(daily_trends) > 1 else day1_performance

        performance_degradation = {
            'trade_latency_change': (day10_performance.get('day_avg_latency', 0) - day1_performance.get('day_avg_latency', 0)),
            'rpc_latency_change': (day10_performance.get('day_rpc_latency', 0) - day1_performance.get('day_rpc_latency', 0)),
            'trade_volume_change': (day10_performance.get('day_trades', 0) - day1_performance.get('day_trades', 0))
        }

        return {
            'hourly_patterns': hourly_patterns,
            'daily_trends': daily_trends,
            'performance_degradation': performance_degradation,
            'key_insights': self._analyze_performance_insights(hourly_patterns, daily_trends)
        }

    def generate_cost_analysis(self) -> Dict[str, Any]:
        """Generate detailed cost analysis.

        Returns:
            Dictionary with cost analysis data
        """
        cursor = self.conn.cursor()

        # Get trade execution details with costs
        cursor.execute('''
            SELECT
                COUNT(*) as total_trades,
                AVG(jito_tip_sol) as avg_jito_tip,
                AVG(dex_fee_sol) as avg_dex_fee,
                AVG(slippage_cost_sol) as avg_slippage,
                AVG(network_fee_sol) as avg_network_fee,
                AVG(total_cost_sol) as avg_total_cost,
                SUM(total_cost_sol) as total_costs
            FROM trade_execution_details
        ''')

        cost_stats = cursor.fetchone()

        # Cost breakdown by strategy
        cursor.execute('''
            SELECT
                strategy,
                AVG(total_cost_sol) as avg_cost,
                COUNT(*) as trade_count
            FROM trade_execution_details
            GROUP BY strategy
        ''')

        strategy_costs = [dict(row) for row in cursor.fetchall()]

        return {
            'overall_costs': {
                'total_trades': cost_stats['total_trades'] or 0,
                'avg_jito_tip_sol': cost_stats['avg_jito_tip'] or 0.0,
                'avg_dex_fee_sol': cost_stats['avg_dex_fee'] or 0.0,
                'avg_slippage_cost_sol': cost_stats['avg_slippage'] or 0.0,
                'avg_network_fee_sol': cost_stats['avg_network_fee'] or 0.0,
                'avg_total_cost_sol': cost_stats['avg_total_cost'] or 0.0,
                'total_costs_sol': cost_stats['total_costs'] or 0.0
            },
            'costs_by_strategy': strategy_costs,
            'cost_optimization_opportunities': self._identify_cost_optimizations(strategy_costs)
        }

    def generate_risk_analysis(self) -> Dict[str, Any]:
        """Generate detailed risk analysis.

        Returns:
            Dictionary with risk analysis data
        """
        cursor = self.conn.cursor()

        # Circuit breaker events
        cursor.execute('''
            SELECT
                COUNT(*) as total_trips,
                AVG(max_drawdown_percent) as avg_drawdown_at_trip,
                MAX(portfolio_exposure_percent) as max_exposure
            FROM evaluation_snapshots
            WHERE circuit_breaker_state = 1
        ''')

        circuit_breaker_stats = cursor.fetchone()

        # Risk metrics by day
        cursor.execute('''
            SELECT
                day_number,
                AVG(max_drawdown_percent) as avg_drawdown,
                MAX(portfolio_exposure_percent) as max_exposure,
                AVG(active_positions_count) as avg_positions
            FROM evaluation_snapshots
            GROUP BY day_number
            ORDER BY day_number
        ''')

        daily_risk_metrics = [dict(row) for row in cursor.fetchall()]

        return {
            'circuit_breaker_analysis': {
                'total_trips': circuit_breaker_stats['total_trips'] or 0,
                'avg_drawdown_at_trip': circuit_breaker_stats['avg_drawdown_at_trip'] or 0.0,
                'max_exposure': circuit_breaker_stats['max_exposure'] or 0.0
            },
            'daily_risk_trends': daily_risk_metrics,
            'risk_assessment': self._assess_overall_risk(daily_risk_metrics, circuit_breaker_stats)
        }

    def generate_system_health(self) -> Dict[str, Any]:
        """Generate system health analysis.

        Returns:
            Dictionary with system health data
        """
        cursor = self.conn.cursor()

        # Overall system health metrics
        cursor.execute('''
            SELECT
                AVG(cpu_usage_percent) as avg_cpu,
                AVG(memory_usage_percent) as avg_memory,
                AVG(disk_usage_percent) as avg_disk,
                SUM(error_count) as total_errors,
                AVG(db_query_latency_avg_ms) as avg_db_latency
            FROM evaluation_snapshots
        ''')

        health_stats = cursor.fetchone()

        # Resource usage trends
        cursor.execute('''
            SELECT
                day_number,
                AVG(cpu_usage_percent) as avg_cpu,
                AVG(memory_usage_percent) as avg_memory,
                AVG(disk_usage_percent) as avg_disk
            FROM evaluation_snapshots
            GROUP BY day_number
            ORDER BY day_number
        ''')

        resource_trends = [dict(row) for row in cursor.fetchall()]

        return {
            'overall_health': {
                'avg_cpu_usage': health_stats['avg_cpu'] or 0.0,
                'avg_memory_usage': health_stats['avg_memory'] or 0.0,
                'avg_disk_usage': health_stats['avg_disk'] or 0.0,
                'total_errors': health_stats['total_errors'] or 0,
                'avg_db_latency': health_stats['avg_db_latency'] or 0.0
            },
            'resource_trends': resource_trends,
            'health_score': self._calculate_health_score(health_stats)
        }

    def get_detected_anomalies(self) -> List[Dict[str, Any]]:
        """Get detected anomalies during evaluation.

        Returns:
            List of anomaly details
        """
        cursor = self.conn.cursor()

        cursor.execute('''
            SELECT
                anomaly_time,
                day_number,
                anomaly_type,
                severity,
                metric_name,
                metric_value,
                threshold_value,
                deviation_percent,
                description,
                resolved
            FROM evaluation_anomalies
            ORDER BY
                CASE severity WHEN 'CRITICAL' THEN 1 WHEN 'WARNING' THEN 2 END,
                deviation_percent DESC
            LIMIT 100
        ''')

        return [dict(row) for row in cursor.fetchall()]

    def generate_recommendations(self) -> List[Dict[str, Any]]:
        """Generate actionable recommendations based on evaluation.

        Returns:
            List of recommendations with priority and impact
        """
        recommendations = []

        # Analyze performance degradation
        performance = self.generate_performance_analysis()
        if performance['performance_degradation']['trade_latency_change'] > 100:
            recommendations.append({
                'category': 'Performance',
                'priority': 'HIGH',
                'title': 'Address Trade Latency Degradation',
                'description': f"Trade latency increased by {performance['performance_degradation']['trade_latency_change']:.1f}ms over evaluation period",
                'impact': 'High',
                'effort': 'Medium',
                'actions': [
                    'Investigate database query optimization',
                    'Review RPC provider performance',
                    'Analyze memory allocation patterns'
                ]
            })

        # Analyze cost efficiency
        costs = self.generate_cost_analysis()
        if costs['overall_costs']['avg_total_cost_sol'] > 0.01:
            recommendations.append({
                'category': 'Cost Optimization',
                'priority': 'MEDIUM',
                'title': 'Reduce Trading Costs',
                'description': f"Average trade cost is {costs['overall_costs']['avg_total_cost_sol']:.4f} SOL",
                'impact': 'Medium',
                'effort': 'Low',
                'actions': [
                    'Optimize Jito tip calculation strategy',
                    'Review DEX fee minimization opportunities',
                    'Analyze slippage reduction techniques'
                ]
            })

        # Analyze risk factors
        risk = self.generate_risk_analysis()
        if risk['circuit_breaker_analysis']['total_trips'] > 3:
            recommendations.append({
                'category': 'Risk Management',
                'priority': 'HIGH',
                'title': 'Reduce Circuit Breaker Trips',
                'description': f"Circuit breaker triggered {risk['circuit_breaker_analysis']['total_trips']} times during evaluation",
                'impact': 'High',
                'effort': 'Medium',
                'actions': [
                    'Review circuit breaker threshold configuration',
                    'Implement early warning system',
                    'Add position sizing limits'
                ]
            })

        return recommendations

    def _get_evaluation_start_date(self) -> str:
        """Get evaluation start date from database."""
        cursor = self.conn.cursor()
        cursor.execute('SELECT MIN(snapshot_time) as start_time FROM evaluation_snapshots')
        result = cursor.fetchone()
        return result['start_time'] if result and result['start_time'] else datetime.now().isoformat()

    def _get_evaluation_end_date(self) -> str:
        """Get evaluation end date from database."""
        cursor = self.conn.cursor()
        cursor.execute('SELECT MAX(snapshot_time) as end_time FROM evaluation_snapshots')
        result = cursor.fetchone()
        return result['end_time'] if result and result['end_time'] else datetime.now().isoformat()

    def _calculate_overall_grade(self, success_rate: float, avg_latency: float, critical_anomalies: int) -> str:
        """Calculate overall system grade.

        Args:
            success_rate: Trade success rate percentage
            avg_latency: Average trade latency in ms
            critical_anomalies: Number of critical anomalies

        Returns:
            Grade letter (A, B, C, D, F)
        """
        score = 100

        # Deduct points for poor success rate
        if success_rate < 95:
            score -= (95 - success_rate) * 2

        # Deduct points for high latency
        if avg_latency > 100:
            score -= (avg_latency - 100) * 0.5

        # Deduct points for critical anomalies
        score -= critical_anomalies * 5

        # Convert to grade
        if score >= 90:
            return 'A'
        elif score >= 80:
            return 'B'
        elif score >= 70:
            return 'C'
        elif score >= 60:
            return 'D'
        else:
            return 'F'

    def _analyze_performance_insights(self, hourly_patterns: List, daily_trends: List) -> List[str]:
        """Generate key performance insights.

        Args:
            hourly_patterns: Hourly performance data
            daily_trends: Daily performance data

        Returns:
            List of insight strings
        """
        insights = []

        if not hourly_patterns or not daily_trends:
            return insights

        # Analyze hourly patterns
        peak_hours = [h for h in hourly_patterns if h['hourly_trades'] > 0]
        if peak_hours:
            max_trades_hour = max(peak_hours, key=lambda x: x['hourly_trades'])
            insights.append(f"Peak trading activity occurs at hour {max_trades_hour['hour_number']} with {max_trades_hour['hourly_trades']} trades")

        # Analyze daily trends
        if len(daily_trends) > 1:
            latency_trend = daily_trends[-1]['day_avg_latency'] - daily_trends[0]['day_avg_latency']
            if latency_trend > 0:
                insights.append(f"Trade latency increased by {latency_trend:.1f}ms from Day 1 to Day {len(daily_trends)}")
            else:
                insights.append(f"Trade latency improved by {abs(latency_trend):.1f}ms from Day 1 to Day {len(daily_trends)}")

        return insights

    def _identify_cost_optimizations(self, strategy_costs: List) -> List[str]:
        """Identify cost optimization opportunities.

        Args:
            strategy_costs: Cost breakdown by strategy

        Returns:
            List of optimization recommendations
        """
        optimizations = []

        for strategy in strategy_costs:
            if strategy['avg_cost'] > 0.02:
                optimizations.append(f"{strategy['strategy'].title()} strategy has high average cost ({strategy['avg_cost']:.4f} SOL)")

        return optimizations

    def _assess_overall_risk(self, daily_metrics: List, circuit_stats: sqlite3.Row) -> Dict[str, Any]:
        """Assess overall risk level.

        Args:
            daily_metrics: Daily risk metrics
            circuit_stats: Circuit breaker statistics

        Returns:
            Risk assessment dictionary
        """
        max_drawdown = max([d['avg_drawdown'] for d in daily_metrics]) if daily_metrics else 0
        circuit_trips = circuit_stats['total_trips'] or 0

        risk_level = 'LOW'
        if max_drawdown > 15 or circuit_trips > 5:
            risk_level = 'HIGH'
        elif max_drawdown > 10 or circuit_trips > 2:
            risk_level = 'MEDIUM'

        return {
            'risk_level': risk_level,
            'max_drawdown_percent': max_drawdown,
            'circuit_breaker_trips': circuit_trips,
            'overall_assessment': f"System operating at {risk_level} risk level"
        }

    def _calculate_health_score(self, health_stats: sqlite3.Row) -> int:
        """Calculate system health score.

        Args:
            health_stats: Overall health statistics

        Returns:
            Health score (0-100)
        """
        score = 100

        # Deduct for high resource usage
        if health_stats['avg_cpu'] > 80:
            score -= (health_stats['avg_cpu'] - 80) * 2

        if health_stats['avg_memory'] > 80:
            score -= (health_stats['avg_memory'] - 80) * 2

        # Deduct for errors
        score -= min(health_stats['total_errors'] or 0, 50)

        return max(0, min(100, score))

    def generate_comprehensive_report(self) -> EvaluationReportData:
        """Generate comprehensive evaluation report.

        Returns:
            EvaluationReportData with all report sections
        """
        print("Generating comprehensive evaluation report...")

        return EvaluationReportData(
            executive_summary=self.generate_executive_summary(),
            performance_analysis=self.generate_performance_analysis(),
            cost_analysis=self.generate_cost_analysis(),
            risk_analysis=self.generate_risk_analysis(),
            system_health=self.generate_system_health(),
            code_profiling={'status': 'Data collection in progress'},
            database_performance={'status': 'Analyzed in system health'},
            network_analysis={'status': 'Network metrics analyzed'},
            anomalies_detected=self.get_detected_anomalies(),
            recommendations=self.generate_recommendations(),
            appendices={
                'data_dictionary': 'See evaluation_schema.sql',
                'investigation_guide': 'Use evaluation database for detailed analysis',
                'raw_data_summary': f'{self.eval_dir}'
            }
        )

    def render_html_report(self, report_data: EvaluationReportData) -> str:
        """Render HTML report from evaluation data.

        Args:
            report_data: Complete evaluation report data

        Returns:
            HTML report content
        """
        # Generate comprehensive HTML report
        html_content = f"""
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Chimera 10-Day Evaluation Report</title>
    <style>
        body {{
            font-family: 'Segoe UI', Tahoma, Geneva, Verdana, sans-serif;
            line-height: 1.6;
            color: #333;
            max-width: 1400px;
            margin: 0 auto;
            padding: 20px;
            background-color: #f5f5f5;
        }}
        .header {{
            background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
            color: white;
            padding: 40px;
            border-radius: 15px;
            margin-bottom: 30px;
            box-shadow: 0 8px 16px rgba(0,0,0,0.2);
        }}
        .header h1 {{
            margin: 0 0 10px 0;
            font-size: 2.8em;
        }}
        .header .grade {{
            font-size: 3em;
            font-weight: bold;
            margin: 20px 0;
        }}
        .grade-A {{ color: #4ade80; }}
        .grade-B {{ color: #60a5fa; }}
        .grade-C {{ color: #fbbf24; }}
        .grade-D {{ color: #f97316; }}
        .grade-F {{ color: #ef4444; }}
        .summary-cards {{
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(250px, 1fr));
            gap: 20px;
            margin-bottom: 30px;
        }}
        .card {{
            background: white;
            padding: 25px;
            border-radius: 10px;
            box-shadow: 0 4px 6px rgba(0,0,0,0.1);
            border-left: 5px solid #667eea;
        }}
        .card h3 {{
            margin: 0 0 15px 0;
            color: #667eea;
            font-size: 1.3em;
        }}
        .card .value {{
            font-size: 2.2em;
            font-weight: bold;
            color: #333;
        }}
        .card .subtitle {{
            color: #666;
            font-size: 0.9em;
            margin-top: 5px;
        }}
        .section {{
            background: white;
            padding: 30px;
            border-radius: 12px;
            margin-bottom: 30px;
            box-shadow: 0 4px 6px rgba(0,0,0,0.1);
        }}
        .section h2 {{
            margin: 0 0 25px 0;
            color: #667eea;
            border-bottom: 3px solid #667eea;
            padding-bottom: 15px;
            font-size: 1.8em;
        }}
        .section h3 {{
            margin: 25px 0 15px 0;
            color: #764ba2;
            font-size: 1.4em;
        }}
        table {{
            width: 100%;
            border-collapse: collapse;
            margin: 20px 0;
        }}
        th, td {{
            padding: 15px;
            text-align: left;
            border-bottom: 1px solid #ddd;
        }}
        th {{
            background-color: #f8f9fa;
            font-weight: bold;
            color: #667eea;
        }}
        tr:hover {{
            background-color: #f5f5f5;
        }}
        .status-good {{ color: #10b981; font-weight: bold; }}
        .status-warning {{ color: #f59e0b; font-weight: bold; }}
        .status-critical {{ color: #ef4444; font-weight: bold; }}
        .recommendation {{
            background: #f0f9ff;
            border-left: 4px solid #0ea5e9;
            padding: 20px;
            margin: 15px 0;
            border-radius: 8px;
        }}
        .recommendation h4 {{
            margin: 0 0 10px 0;
            color: #0ea5e9;
        }}
        .recommendation .priority {{
            display: inline-block;
            padding: 4px 12px;
            border-radius: 20px;
            font-size: 0.85em;
            font-weight: bold;
            margin-right: 10px;
        }}
        .priority-HIGH {{ background: #fef2f2; color: #dc2626; }}
        .priority-MEDIUM {{ background: #fef3c7; color: #d97706; }}
        .priority-LOW {{ background: #d1fae5; color: #059669; }}
        .insights {{
            background: #f0fdf4;
            border-left: 4px solid #22c55e;
            padding: 20px;
            margin: 20px 0;
            border-radius: 8px;
        }}
        .insights h4 {{
            margin: 0 0 15px 0;
            color: #16a34a;
        }}
        .footer {{
            text-align: center;
            color: #666;
            margin-top: 40px;
            padding: 30px;
            border-top: 2px solid #ddd;
        }}
    </style>
</head>
<body>
    <div class="header">
        <h1>🔬 Chimera 10-Day Evaluation Report</h1>
        <div class="subtitle">Comprehensive System Analysis & Performance Assessment</div>
        <div class="grade grade-{report_data.executive_summary['overall_grade']}">
            Overall Grade: {report_data.executive_summary['overall_grade']}
        </div>
        <div class="timestamp">
            Report Generated: {datetime.now().strftime('%B %d, %Y at %H:%M:%S')}<br/>
            Evaluation Period: {report_data.executive_summary['evaluation_period']['start_date']} to {report_data.executive_summary['evaluation_period']['end_date']}<br/>
            Days Evaluated: {report_data.executive_summary['evaluation_period']['days_evaluated']}
        </div>
    </div>

    <div class="summary-cards">
        <div class="card">
            <h3>Total Trades</h3>
            <div class="value">{report_data.executive_summary['trading_performance']['total_trades']:,}</div>
            <div class="subtitle">Trading Activity</div>
        </div>
        <div class="card">
            <h3>Success Rate</h3>
            <div class="value">{report_data.executive_summary['trading_performance']['success_rate_percent']:.1f}%</div>
            <div class="subtitle">Trade Success</div>
        </div>
        <div class="card">
            <h3>Total PnL</h3>
            <div class="value">{report_data.executive_summary['trading_performance']['net_pnl_sol']:.4f} SOL</div>
            <div class="subtitle">Net Performance</div>
        </div>
        <div class="card">
            <h3>Avg Latency</h3>
            <div class="value">{report_data.executive_summary['system_performance']['avg_trade_latency_ms']:.1f}ms</div>
            <div class="subtitle">Trade Execution</div>
        </div>
        <div class="card">
            <h3>Health Score</h3>
            <div class="value">{report_data.system_health['health_score']}/100</div>
            <div class="subtitle">System Health</div>
        </div>
        <div class="card">
            <h3>Active Anomalies</h3>
            <div class="value">{report_data.executive_summary['system_performance']['total_anomalies']}</div>
            <div class="subtitle">Issues Detected</div>
        </div>
    </div>

    <div class="section">
        <h2>📊 Performance Analysis</h2>
        <h3>Trading Performance</h3>
        <table>
            <tr><th>Metric</th><th>Value</th><th>Status</th></tr>
            <tr>
                <td>Total Trades</td>
                <td>{report_data.executive_summary['trading_performance']['total_trades']:,}</td>
                <td class="status-good">Active</td>
            </tr>
            <tr>
                <td>Success Rate</td>
                <td>{report_data.executive_summary['trading_performance']['success_rate_percent']:.1f}%</td>
                <td class="{'status-good' if report_data.executive_summary['trading_performance']['success_rate_percent'] >= 95 else 'status-warning'}">
                    {'Excellent' if report_data.executive_summary['trading_performance']['success_rate_percent'] >= 95 else 'Review'}
                </td>
            </tr>
            <tr>
                <td>Average Trade Latency</td>
                <td>{report_data.executive_summary['system_performance']['avg_trade_latency_ms']:.1f}ms</td>
                <td class="{'status-good' if report_data.executive_summary['system_performance']['avg_trade_latency_ms'] < 100 else 'status-warning'}">
                    {'Good' if report_data.executive_summary['system_performance']['avg_trade_latency_ms'] < 100 else 'High'}
                </td>
            </tr>
            <tr>
                <td>Net PnL</td>
                <td>{report_data.executive_summary['trading_performance']['net_pnl_sol']:.4f} SOL</td>
                <td class="{'status-good' if report_data.executive_summary['trading_performance']['net_pnl_sol'] >= 0 else 'status-critical'}">
                    {'Profitable' if report_data.executive_summary['trading_performance']['net_pnl_sol'] >= 0 else 'Loss'}
                </td>
            </tr>
        </table>

        <h3>Performance Insights</h3>
        <div class="insights">
            <h4>🔍 Key Findings</h4>
            <ul>
                {"".join(f"<li>{insight}</li>" for insight in report_data.performance_analysis['key_insights'])}
            </ul>
        </div>
    </div>

    <div class="section">
        <h2>💰 Cost Analysis</h2>
        <h3>Trading Costs</h3>
        <table>
            <tr><th>Cost Component</th><th>Average (SOL)</th><th>Total (SOL)</th></tr>
            <tr>
                <td>Jito Tips</td>
                <td>{report_data.cost_analysis['overall_costs']['avg_jito_tip_sol']:.6f}</td>
                <td>{report_data.cost_analysis['overall_costs']['avg_jito_tip_sol'] * report_data.cost_analysis['overall_costs']['total_trades']:.4f}</td>
            </tr>
            <tr>
                <td>DEX Fees</td>
                <td>{report_data.cost_analysis['overall_costs']['avg_dex_fee_sol']:.6f}</td>
                <td>{report_data.cost_analysis['overall_costs']['avg_dex_fee_sol'] * report_data.cost_analysis['overall_costs']['total_trades']:.4f}</td>
            </tr>
            <tr>
                <td>Slippage</td>
                <td>{report_data.cost_analysis['overall_costs']['avg_slippage_cost_sol']:.6f}</td>
                <td>{report_data.cost_analysis['overall_costs']['avg_slippage_cost_sol'] * report_data.cost_analysis['overall_costs']['total_trades']:.4f}</td>
            </tr>
            <tr>
                <td><strong>Total Cost per Trade</strong></td>
                <td><strong>{report_data.cost_analysis['overall_costs']['avg_total_cost_sol']:.6f}</strong></td>
                <td><strong>{report_data.cost_analysis['overall_costs']['total_costs_sol']:.4f}</strong></td>
            </tr>
        </table>
    </div>

    <div class="section">
        <h2>⚠️ Risk Analysis</h2>
        <h3>Risk Assessment</h3>
        <table>
            <tr><th>Risk Metric</th><th>Value</th><th>Status</th></tr>
            <tr>
                <td>Risk Level</td>
                <td>{report_data.risk_analysis['risk_assessment']['risk_level']}</td>
                <td class="{'status-good' if report_data.risk_analysis['risk_assessment']['risk_level'] == 'LOW' else 'status-warning'}">
                    {report_data.risk_analysis['risk_assessment']['risk_level']}
                </td>
            </tr>
            <tr>
                <td>Max Drawdown</td>
                <td>{report_data.risk_analysis['risk_assessment']['max_drawdown_percent']:.1f}%</td>
                <td class="{'status-good' if report_data.risk_analysis['risk_assessment']['max_drawdown_percent'] < 10 else 'status-warning'}">
                    {'Acceptable' if report_data.risk_analysis['risk_assessment']['max_drawdown_percent'] < 10 else 'High'}
                </td>
            </tr>
            <tr>
                <td>Circuit Breaker Trips</td>
                <td>{report_data.risk_analysis['risk_assessment']['circuit_breaker_trips']}</td>
                <td class="{'status-good' if report_data.risk_analysis['risk_assessment']['circuit_breaker_trips'] < 3 else 'status-warning'}">
                    {'Normal' if report_data.risk_analysis['risk_assessment']['circuit_breaker_trips'] < 3 else 'Frequent'}
                </td>
            </tr>
        </table>
    </div>

    <div class="section">
        <h2>🔧 System Health</h2>
        <h3>Resource Usage</h3>
        <table>
            <tr><th>Resource</th><th>Average Usage</th><th>Status</th></tr>
            <tr>
                <td>CPU</td>
                <td>{report_data.system_health['overall_health']['avg_cpu_usage']:.1f}%</td>
                <td class="{'status-good' if report_data.system_health['overall_health']['avg_cpu_usage'] < 80 else 'status-warning'}">
                    {'Good' if report_data.system_health['overall_health']['avg_cpu_usage'] < 80 else 'High'}
                </td>
            </tr>
            <tr>
                <td>Memory</td>
                <td>{report_data.system_health['overall_health']['avg_memory_usage']:.1f}%</td>
                <td class="{'status-good' if report_data.system_health['overall_health']['avg_memory_usage'] < 80 else 'status-warning'}">
                    {'Good' if report_data.system_health['overall_health']['avg_memory_usage'] < 80 else 'High'}
                </td>
            </tr>
            <tr>
                <td>Total Errors</td>
                <td>{report_data.system_health['overall_health']['total_errors']:,}</td>
                <td class="{'status-good' if report_data.system_health['overall_health']['total_errors'] < 100 else 'status-warning'}">
                    {'Low' if report_data.system_health['overall_health']['total_errors'] < 100 else 'Elevated'}
                </td>
            </tr>
        </table>
    </div>

    <div class="section">
        <h2>🎯 Recommendations</h2>
        {"".join(f"""
        <div class="recommendation">
            <h4><span class="priority-{rec['priority']}">{rec['priority']}</span> {rec['title']}</h4>
            <p><strong>Description:</strong> {rec['description']}</p>
            <p><strong>Impact:</strong> {rec['impact']} | <strong>Effort:</strong> {rec['effort']}</p>
            <p><strong>Actions:</strong></p>
            <ul>
                {"".join(f"<li>{action}</li>" for action in rec['actions'])}
            </ul>
        </div>
        """ for rec in report_data.recommendations)}
    </div>

    <div class="section">
        <h2>⚠️ Detected Anomalies (Top 20)</h2>
        <table>
            <tr><th>Time</th><th>Severity</th><th>Metric</th><th>Value</th><th>Threshold</th><th>Status</th></tr>
            {"".join(f"""
            <tr>
                <td>{anomaly['anomaly_time'][:19]}</td>
                <td class="{'status-critical' if anomaly['severity'] == 'CRITICAL' else 'status-warning'}">{anomaly['severity']}</td>
                <td>{anomaly['metric_name']}</td>
                <td>{anomaly['metric_value']:.2f}</td>
                <td>{anomaly['threshold_value']:.2f}</td>
                <td>{'Resolved' if anomaly['resolved'] else 'Active'}</td>
            </tr>
            """ for anomaly in report_data.anomalies_detected[:20])}
        </table>
    </div>

    <div class="footer">
        <h3>Chimera Evaluation System</h3>
        <p>Report ID: {datetime.now().strftime('%Y%m%d%H%M%S')}</p>
        <p>Generated by Chimera Evaluation Framework v1.0</p>
        <p>Evaluation Directory: {self.eval_dir}</p>
        <p>Database: {self.db_path}</p>
    </div>
</body>
</html>
        """

        return html_content


def main():
    """Main entry point for report generation."""
    parser = argparse.ArgumentParser(
        description='Generate comprehensive evaluation report'
    )
    parser.add_argument(
        '--evaluation-dir',
        type=str,
        default='/opt/chimera/evaluation',
        help='Evaluation directory path'
    )
    parser.add_argument(
        '--database',
        type=str,
        default='/opt/chimera/evaluation/evaluation.db',
        help='Evaluation database path'
    )
    parser.add_argument(
        '--output',
        type=str,
        default=None,
        help='Output HTML file path (default: evaluation-dir/FINAL_EVALUATION_REPORT.html)'
    )

    args = parser.parse_args()

    # Validate inputs
    eval_dir = Path(args.evaluation_dir)
    db_path = Path(args.database)

    if not db_path.exists():
        print(f"Error: Database not found: {db_path}")
        sys.exit(1)

    if not eval_dir.exists():
        print(f"Error: Evaluation directory not found: {eval_dir}")
        sys.exit(1)

    # Set output path
    output_path = Path(args.output) if args.output else eval_dir / 'FINAL_EVALUATION_REPORT.html'

    # Generate report
    print("=" * 60)
    print("Chimera Evaluation Report Generator")
    print("=" * 60)
    print(f"Evaluation Directory: {eval_dir}")
    print(f"Database: {db_path}")
    print(f"Output: {output_path}")
    print("")

    generator = EvaluationReportGenerator(str(db_path), eval_dir)
    generator.connect()

    try:
        # Generate comprehensive report
        report_data = generator.generate_comprehensive_report()
        html_content = generator.render_html_report(report_data)

        # Save report
        with open(output_path, 'w') as f:
            f.write(html_content)

        print(f"✓ Report generated successfully: {output_path}")
        print(f"  Report size: {len(html_content):,} bytes")
        print("")
        print("Report Summary:")
        print(f"  Overall Grade: {report_data.executive_summary['overall_grade']}")
        print(f"  Total Trades: {report_data.executive_summary['trading_performance']['total_trades']:,}")
        print(f"  Success Rate: {report_data.executive_summary['trading_performance']['success_rate_percent']:.1f}%")
        print(f"  Health Score: {report_data.system_health['health_score']}/100")
        print(f"  Recommendations: {len(report_data.recommendations)}")
        print("")

    except Exception as e:
        print(f"Error generating report: {e}")
        sys.exit(1)

    finally:
        generator.close()

    sys.exit(0)


if __name__ == '__main__':
    main()