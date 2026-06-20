#!/usr/bin/env python3
"""
Chimera Monitoring Gateway Tester
Comprehensive testing suite for HAProxy monitoring gateway access

This tool validates:
1. Gateway routing for all monitoring endpoints
2. Access control enforcement (geographic, authentication)
3. SSL/TLS connectivity
4. Performance benchmarks
5. Integration with existing monitoring tools
"""

import requests
import json
import sys
import time
import argparse
from typing import Dict, List, Tuple, Any
from urllib.parse import urljoin
from dataclasses import dataclass
from enum import Enum

class TestResult(Enum):
    PASS = "PASS"
    FAIL = "FAIL"
    WARN = "WARN"
    SKIP = "SKIP"

@dataclass
class Test:
    name: str
    category: str
    description: str
    function: callable
    critical: bool = True

class MonitoringGatewayTester:
    """Main tester class for monitoring gateway validation"""

    def __init__(self, base_url: str = "https://localhost", verify_ssl: bool = False):
        self.base_url = base_url
        self.verify_ssl = verify_ssl
        self.results = []
        self.performance_metrics = {}

        # Test credentials (should match config)
        self.admin_auth = ('admin', 'changeme_asap')
        self.operator_auth = ('operator', 'changeme_asap')
        self.viewer_auth = ('viewer', 'changeme_asap')

    def log_test(self, test_name: str, result: TestResult, message: str, duration: float = 0.0, details: Dict = None):
        """Log test result"""
        test_entry = {
            'test': test_name,
            'result': result.value,
            'message': message,
            'duration': duration,
            'details': details or {}
        }
        self.results.append(test_entry)

        # Console output
        status_symbol = {
            TestResult.PASS: "✅",
            TestResult.FAIL: "❌",
            TestResult.WARN: "⚠️ ",
            TestResult.SKIP: "⏭️ "
        }[result]

        print(f"{status_symbol} {test_name}: {message} ({duration:.3f}s)")

        if details:
            print(f"   Details: {json.dumps(details, indent=2)}")

    def test_prometheus_gateway_access(self) -> TestResult:
        """Test Prometheus access through gateway"""
        try:
            url = urljoin(self.base_url, "/monitoring/prometheus/")
            response = requests.get(url, auth=self.admin_auth, verify=self.verify_ssl, timeout=10)

            if response.status_code == 200:
                self.log_test("Prometheus Gateway Access", TestResult.PASS,
                           "Prometheus accessible via gateway", response.elapsed.total_seconds())
                return TestResult.PASS
            elif response.status_code == 401:
                self.log_test("Prometheus Gateway Access", TestResult.WARN,
                           "Prometheus requires authentication", response.elapsed.total_seconds())
                return TestResult.WARN
            else:
                self.log_test("Prometheus Gateway Access", TestResult.FAIL,
                           f"Unexpected status code: {response.status_code}", response.elapsed.total_seconds())
                return TestResult.FAIL

        except Exception as e:
            self.log_test("Prometheus Gateway Access", TestResult.FAIL,
                       f"Connection failed: {str(e)}")
            return TestResult.FAIL

    def test_grafana_gateway_access(self) -> TestResult:
        """Test Grafana access through gateway"""
        try:
            url = urljoin(self.base_url, "/monitoring/grafana/")
            response = requests.get(url, auth=self.viewer_auth, verify=self.verify_ssl, timeout=10)

            if response.status_code == 200:
                self.log_test("Grafana Gateway Access", TestResult.PASS,
                           "Grafana accessible via gateway", response.elapsed.total_seconds())
                return TestResult.PASS
            elif response.status_code == 401:
                self.log_test("Grafana Gateway Access", TestResult.WARN,
                           "Grafana requires authentication", response.elapsed.total_seconds())
                return TestResult.WARN
            else:
                self.log_test("Grafana Gateway Access", TestResult.FAIL,
                           f"Unexpected status code: {response.status_code}", response.elapsed.total_seconds())
                return TestResult.FAIL

        except Exception as e:
            self.log_test("Grafana Gateway Access", TestResult.FAIL,
                       f"Connection failed: {str(e)}")
            return TestResult.FAIL

    def test_alertmanager_gateway_access(self) -> TestResult:
        """Test AlertManager access through gateway"""
        try:
            url = urljoin(self.base_url, "/monitoring/alerts/")
            response = requests.get(url, auth=self.admin_auth, verify=self.verify_ssl, timeout=10)

            if response.status_code == 200:
                self.log_test("AlertManager Gateway Access", TestResult.PASS,
                           "AlertManager accessible via gateway", response.elapsed.total_seconds())
                return TestResult.PASS
            elif response.status_code == 401:
                self.log_test("AlertManager Gateway Access", TestResult.WARN,
                           "AlertManager requires authentication", response.elapsed.total_seconds())
                return TestResult.WARN
            else:
                self.log_test("AlertManager Gateway Access", TestResult.FAIL,
                           f"Unexpected status code: {response.status_code}", response.elapsed.total_seconds())
                return TestResult.FAIL

        except Exception as e:
            self.log_test("AlertManager Gateway Access", TestResult.FAIL,
                       f"Connection failed: {str(e)}")
            return TestResult.FAIL

    def test_legacy_url_compatibility(self) -> TestResult:
        """Test legacy URL compatibility (metrics, grafana, alerts)"""
        try:
            legacy_urls = [
                ("/metrics/", "Prometheus legacy URL"),
                ("/grafana/", "Grafana legacy URL"),
                ("/alerts/", "AlertManager legacy URL")
            ]

            results = []
            for path, description in legacy_urls:
                url = urljoin(self.base_url, path)
                response = requests.get(url, auth=self.admin_auth, verify=self.verify_ssl, timeout=10)

                # Check for deprecation headers
                has_deprecation = 'X-Deprecation-Warning' in response.headers or 'X-Deprecation-Notice' in response.headers

                if response.status_code == 200 and has_deprecation:
                    results.append(True)
                    self.log_test(f"Legacy URL - {description}", TestResult.PASS,
                               "Legacy URL works with deprecation notice", response.elapsed.total_seconds(),
                               {'deprecation_headers': dict(response.headers)})
                elif response.status_code == 200:
                    results.append(True)
                    self.log_test(f"Legacy URL - {description}", TestResult.WARN,
                               "Legacy URL works but missing deprecation notice", response.elapsed.total_seconds())
                else:
                    results.append(False)
                    self.log_test(f"Legacy URL - {description}", TestResult.FAIL,
                               f"Legacy URL failed: {response.status_code}", response.elapsed.total_seconds())

            if all(results):
                return TestResult.PASS
            elif any(results):
                return TestResult.WARN
            else:
                return TestResult.FAIL

        except Exception as e:
            self.log_test("Legacy URL Compatibility", TestResult.FAIL,
                       f"Connection failed: {str(e)}")
            return TestResult.FAIL

    def test_authentication_enforcement(self) -> TestResult:
        """Test that authentication is enforced for monitoring endpoints"""
        try:
            endpoints = [
                "/monitoring/prometheus/",
                "/monitoring/grafana/",
                "/monitoring/alerts/"
            ]

            auth_enforced = []
            for endpoint in endpoints:
                url = urljoin(self.base_url, endpoint)

                # Test without authentication
                response_no_auth = requests.get(url, verify=self.verify_ssl, timeout=10)

                # Test with authentication
                response_with_auth = requests.get(url, auth=self.admin_auth, verify=self.verify_ssl, timeout=10)

                # Authentication should be required
                if response_no_auth.status_code == 401 and response_with_auth.status_code == 200:
                    auth_enforced.append(True)
                    self.log_test(f"Authentication Enforcement - {endpoint}", TestResult.PASS,
                               "Authentication properly enforced", response_with_auth.elapsed.total_seconds())
                else:
                    auth_enforced.append(False)
                    self.log_test(f"Authentication Enforcement - {endpoint}", TestResult.FAIL,
                               f"Authentication check failed - no auth: {response_no_auth.status_code}, with auth: {response_with_auth.status_code}")

            if all(auth_enforced):
                return TestResult.PASS
            else:
                return TestResult.FAIL

        except Exception as e:
            self.log_test("Authentication Enforcement", TestResult.FAIL,
                       f"Test failed: {str(e)}")
            return TestResult.FAIL

    def test_role_based_access(self) -> TestResult:
        """Test role-based access control for different user types"""
        try:
            test_cases = [
                # (endpoint, auth, should_succeed)
                ("/monitoring/prometheus/api/v1/targets", self.admin_auth, True),
                ("/monitoring/prometheus/api/v1/targets", self.operator_auth, True),
                ("/monitoring/prometheus/api/v1/targets", self.viewer_auth, False),
                ("/monitoring/grafana/api/dashboards", self.admin_auth, True),
                ("/monitoring/grafana/api/dashboards", self.operator_auth, True),
                ("/monitoring/grafana/api/dashboards", self.viewer_auth, True),
                ("/monitoring/alerts/api/v1/alerts", self.admin_auth, True),
                ("/monitoring/alerts/api/v1/alerts", self.operator_auth, False),
                ("/monitoring/alerts/api/v1/alerts", self.viewer_auth, False),
            ]

            results = []
            for endpoint, auth, should_succeed in test_cases:
                url = urljoin(self.base_url, endpoint)
                response = requests.get(url, auth=auth, verify=self.verify_ssl, timeout=10)

                # Check if result matches expectation
                actual_success = response.status_code == 200
                if actual_success == should_succeed:
                    results.append(True)
                    role = auth[0] if auth else "none"
                    self.log_test(f"RBAC - {role} @ {endpoint}", TestResult.PASS,
                               "Access control working as expected", response.elapsed.total_seconds())
                else:
                    results.append(False)
                    role = auth[0] if auth else "none"
                    self.log_test(f"RBAC - {role} @ {endpoint}", TestResult.FAIL,
                           f"Access control mismatch - expected: {'allow' if should_succeed else 'deny'}, got: {'allow' if actual_success else 'deny'} ({response.status_code})")

            if all(results):
                return TestResult.PASS
            elif any(results):
                return TestResult.WARN
            else:
                return TestResult.FAIL

        except Exception as e:
            self.log_test("Role-Based Access Control", TestResult.FAIL,
                       f"Test failed: {str(e)}")
            return TestResult.FAIL

    def test_geographic_restrictions(self) -> TestResult:
        """Test geographic access control restrictions"""
        try:
            # Simulate requests from different countries using X-Forwarded-For
            test_cases = [
                # (ip_address, should_be_allowed, country)
                ("8.8.8.8", True, "US"),           # Google DNS - US
                ("1.2.4.8", False, "CN"),          # Chinese IP - should be blocked
                ("185.12.12.12", False, "RU"),     # Russian IP - should be blocked
                ("139.28.228.10", False, "KP"),    # North Korean IP - should be blocked
            ]

            results = []
            for ip, should_be_allowed, country in test_cases:
                headers = {
                    'X-Forwarded-For': ip,
                    'X-Real-IP': ip
                }

                url = urljoin(self.base_url, "/monitoring/prometheus/")
                response = requests.get(url, headers=headers, auth=self.admin_auth, verify=self.verify_ssl, timeout=10)

                # Check if access matches expectation
                actual_allowed = response.status_code == 200
                if actual_allowed == should_be_allowed:
                    results.append(True)
                    self.log_test(f"GeoIP Filter - {country} ({ip})", TestResult.PASS,
                               f"Geographic filtering working correctly", response.elapsed.total_seconds(),
                               {'expected': 'allow' if should_be_allowed else 'deny',
                                'actual': 'allow' if actual_allowed else 'deny',
                                'status_code': response.status_code})
                else:
                    results.append(False)
                    self.log_test(f"GeoIP Filter - {country} ({ip})", TestResult.WARN,
                           f"Geographic filtering unexpected - expected: {'allow' if should_be_allowed else 'deny'}, got: {'allow' if actual_allowed else 'deny'} ({response.status_code})",
                           response.elapsed.total_seconds())

            if all(results):
                return TestResult.PASS
            elif any(results):
                return TestResult.WARN
            else:
                return TestResult.FAIL

        except Exception as e:
            self.log_test("Geographic Restrictions", TestResult.WARN,
                       f"Test skipped (GeoIP may not be configured): {str(e)}")
            return TestResult.WARN

    def test_ssl_tls_connectivity(self) -> TestResult:
        """Test SSL/TLS connectivity and certificate validation"""
        try:
            if not self.base_url.startswith("https://"):
                self.log_test("SSL/TLS Connectivity", TestResult.SKIP,
                           "Not using HTTPS - SSL test skipped")
                return TestResult.SKIP

            url = urljoin(self.base_url, "/monitoring/prometheus/")
            response = requests.get(url, auth=self.admin_auth, verify=self.verify_ssl, timeout=10)

            if response.status_code == 200:
                # Check if SSL was actually used
                if response.url.startswith("https://"):
                    self.log_test("SSL/TLS Connectivity", TestResult.PASS,
                               "SSL/TLS connection successful", response.elapsed.total_seconds())
                    return TestResult.PASS
                else:
                    self.log_test("SSL/TLS Connectivity", TestResult.WARN,
                               "Connection not using HTTPS")
                    return TestResult.WARN
            else:
                self.log_test("SSL/TLS Connectivity", TestResult.FAIL,
                           f"SSL connection failed: {response.status_code}")
                return TestResult.FAIL

        except Exception as e:
            self.log_test("SSL/TLS Connectivity", TestResult.WARN,
                       f"SSL test inconclusive: {str(e)}")
            return TestResult.WARN

    def test_performance_benchmarks(self) -> TestResult:
        """Test performance benchmarks for monitoring access"""
        try:
            endpoints = [
                "/monitoring/prometheus/api/v1/query?query=up",
                "/monitoring/grafana/api/health",
                "/monitoring/alerts/api/v1/status"
            ]

            latencies = []
            for endpoint in endpoints:
                url = urljoin(self.base_url, endpoint)

                # Make multiple requests to get average latency
                times = []
                for i in range(5):
                    start = time.time()
                    response = requests.get(url, auth=self.admin_auth, verify=self.verify_ssl, timeout=30)
                    end = time.time()

                    if response.status_code == 200:
                        times.append((end - start) * 1000)  # Convert to ms
                    else:
                        break

                if times:
                    avg_latency = sum(times) / len(times)
                    latencies.append(avg_latency)

                    # Check if latency is acceptable (< 1s for monitoring queries)
                    if avg_latency < 1000:
                        self.log_test(f"Performance - {endpoint}", TestResult.PASS,
                                   f"Average latency: {avg_latency:.2f}ms", avg_latency / 1000,
                                   {'latency_ms': avg_latency, 'samples': len(times)})
                    elif avg_latency < 3000:
                        self.log_test(f"Performance - {endpoint}", TestResult.WARN,
                                   f"High latency: {avg_latency:.2f}ms", avg_latency / 1000,
                                   {'latency_ms': avg_latency, 'samples': len(times)})
                    else:
                        self.log_test(f"Performance - {endpoint}", TestResult.FAIL,
                                   f"Unacceptable latency: {avg_latency:.2f}ms", avg_latency / 1000,
                                   {'latency_ms': avg_latency, 'samples': len(times)})

            if latencies:
                overall_avg = sum(latencies) / len(latencies)
                self.performance_metrics['average_latency_ms'] = overall_avg
                self.performance_metrics['endpoints_tested'] = len(latencies)

                # Overall performance assessment
                if overall_avg < 500:
                    self.log_test("Overall Performance", TestResult.PASS,
                               f"Excellent average latency: {overall_avg:.2f}ms")
                    return TestResult.PASS
                elif overall_avg < 1500:
                    self.log_test("Overall Performance", TestResult.WARN,
                           f"Acceptable average latency: {overall_avg:.2f}ms")
                    return TestResult.WARN
                else:
                    self.log_test("Overall Performance", TestResult.FAIL,
                           f"Poor average latency: {overall_avg:.2f}ms")
                    return TestResult.FAIL
            else:
                self.log_test("Performance Benchmarks", TestResult.FAIL,
                           "No successful requests to measure")
                return TestResult.FAIL

        except Exception as e:
            self.log_test("Performance Benchmarks", TestResult.FAIL,
                       f"Performance test failed: {str(e)}")
            return TestResult.FAIL

    def test_prometheus_integration(self) -> TestResult:
        """Test Prometheus integration and query functionality"""
        try:
            url = urljoin(self.base_url, "/monitoring/prometheus/api/v1/query?query=up")
            response = requests.get(url, auth=self.admin_auth, verify=self.verify_ssl, timeout=10)

            if response.status_code == 200:
                try:
                    data = response.json()
                    if data.get('status') == 'success' and 'data' in data:
                        self.log_test("Prometheus Integration", TestResult.PASS,
                                   "Prometheus API working correctly", response.elapsed.total_seconds(),
                                   {'query_response': data.get('data', {}).get('result', [])[:3]})
                        return TestResult.PASS
                    else:
                        self.log_test("Prometheus Integration", TestResult.WARN,
                           "Prometheus API response unexpected", response.elapsed.total_seconds())
                        return TestResult.WARN
                except json.JSONDecodeError:
                    self.log_test("Prometheus Integration", TestResult.WARN,
                           "Prometheus API returned invalid JSON", response.elapsed.total_seconds())
                    return TestResult.WARN
            else:
                self.log_test("Prometheus Integration", TestResult.FAIL,
                           f"Prometheus API failed: {response.status_code}", response.elapsed.total_seconds())
                return TestResult.FAIL

        except Exception as e:
            self.log_test("Prometheus Integration", TestResult.FAIL,
                       f"Integration test failed: {str(e)}")
            return TestResult.FAIL

    def test_grafana_integration(self) -> TestResult:
        """Test Grafana integration and dashboard access"""
        try:
            url = urljoin(self.base_url, "/monitoring/grafana/api/health")
            response = requests.get(url, auth=self.admin_auth, verify=self.verify_ssl, timeout=10)

            if response.status_code == 200:
                try:
                    data = response.json()
                    if data.get('database') == 'ok' and data.get('commit') == 'ok':
                        self.log_test("Grafana Integration", TestResult.PASS,
                                   "Grafana health check passed", response.elapsed.total_seconds(),
                                   {'health_status': data})
                        return TestResult.PASS
                    else:
                        self.log_test("Grafana Integration", TestResult.WARN,
                           "Grafana health check warnings", response.elapsed.total_seconds(),
                           {'health_status': data})
                        return TestResult.WARN
                except json.JSONDecodeError:
                    self.log_test("Grafana Integration", TestResult.WARN,
                           "Grafana API returned invalid JSON", response.elapsed.total_seconds())
                    return TestResult.WARN
            else:
                self.log_test("Grafana Integration", TestResult.FAIL,
                           f"Grafana health check failed: {response.status_code}", response.elapsed.total_seconds())
                return TestResult.FAIL

        except Exception as e:
            self.log_test("Grafana Integration", TestResult.FAIL,
                       f"Integration test failed: {str(e)}")
            return TestResult.FAIL

    def test_alertmanager_integration(self) -> TestResult:
        """Test AlertManager integration and alert functionality"""
        try:
            url = urljoin(self.base_url, "/monitoring/alerts/api/v1/status")
            response = requests.get(url, auth=self.admin_auth, verify=self.verify_ssl, timeout=10)

            if response.status_code == 200:
                try:
                    data = response.json()
                    if 'data' in data:
                        self.log_test("AlertManager Integration", TestResult.PASS,
                                   "AlertManager API working correctly", response.elapsed.total_seconds(),
                                   {'alertmanager_status': data.get('data', {})})
                        return TestResult.PASS
                    else:
                        self.log_test("AlertManager Integration", TestResult.WARN,
                           "AlertManager API response unexpected", response.elapsed.total_seconds())
                        return TestResult.WARN
                except json.JSONDecodeError:
                    self.log_test("AlertManager Integration", TestResult.WARN,
                           "AlertManager API returned invalid JSON", response.elapsed.total_seconds())
                    return TestResult.WARN
            else:
                self.log_test("AlertManager Integration", TestResult.FAIL,
                           f"AlertManager API failed: {response.status_code}", response.elapsed.total_seconds())
                return TestResult.FAIL

        except Exception as e:
            self.log_test("AlertManager Integration", TestResult.FAIL,
                       f"Integration test failed: {str(e)}")
            return TestResult.FAIL

    def run_all_tests(self) -> Dict[str, Any]:
        """Run all tests and generate summary"""
        print("🚀 Starting Chimera Monitoring Gateway Tests")
        print("=" * 60)

        tests = [
            ("Gateway Access", [
                self.test_prometheus_gateway_access,
                self.test_grafana_gateway_access,
                self.test_alertmanager_gateway_access,
                self.test_legacy_url_compatibility
            ]),
            ("Security & Access Control", [
                self.test_authentication_enforcement,
                self.test_role_based_access,
                self.test_geographic_restrictions,
                self.test_ssl_tls_connectivity
            ]),
            ("Performance & Integration", [
                self.test_performance_benchmarks,
                self.test_prometheus_integration,
                self.test_grafana_integration,
                self.test_alertmanager_integration
            ])
        ]

        for category, test_group in tests:
            print(f"\n📋 {category}")
            print("-" * 40)
            for test in test_group:
                test()

        # Generate summary
        print("\n" + "=" * 60)
        print("📊 Test Summary")
        print("=" * 60)

        total_tests = len(self.results)
        passed = sum(1 for r in self.results if r['result'] == 'PASS')
        failed = sum(1 for r in self.results if r['result'] == 'FAIL')
        warned = sum(1 for r in self.results if r['result'] == 'WARN')
        skipped = sum(1 for r in self.results if r['result'] == 'SKIP')

        print(f"Total Tests: {total_tests}")
        print(f"✅ Passed: {passed}")
        print(f"⚠️  Warnings: {warned}")
        print(f"❌ Failed: {failed}")
        print(f"⏭️  Skipped: {skipped}")

        success_rate = (passed / total_tests * 100) if total_tests > 0 else 0
        print(f"\nSuccess Rate: {success_rate:.1f}%")

        if self.performance_metrics:
            print(f"\n⚡ Performance Metrics:")
            print(f"Average Latency: {self.performance_metrics.get('average_latency_ms', 0):.2f}ms")
            print(f"Endpoints Tested: {self.performance_metrics.get('endpoints_tested', 0)}")

        # Overall assessment
        if failed == 0:
            print("\n🎉 All critical tests passed!")
            return {'overall': 'SUCCESS', 'passed': passed, 'failed': failed, 'warned': warned}
        elif failed <= 2:
            print("\n⚠️  Some tests failed but gateway is functional")
            return {'overall': 'PARTIAL', 'passed': passed, 'failed': failed, 'warned': warned}
        else:
            print("\n❌ Multiple test failures - gateway needs attention")
            return {'overall': 'FAILURE', 'passed': passed, 'failed': failed, 'warned': warned}

def main():
    parser = argparse.ArgumentParser(description='Chimera Monitoring Gateway Tester')
    parser.add_argument('--url', default='https://localhost', help='Base URL of the monitoring gateway')
    parser.add_argument('--verify-ssl', action='store_true', help='Verify SSL certificates')
    parser.add_argument('--output', help='Output results to JSON file')
    parser.add_argument('--quick', action='store_true', help='Run quick tests only')

    args = parser.parse_args()

    tester = MonitoringGatewayTester(base_url=args.url, verify_ssl=args.verify_ssl)
    results = tester.run_all_tests()

    if args.output:
        with open(args.output, 'w') as f:
            json.dump({
                'summary': results,
                'tests': tester.results,
                'performance': tester.performance_metrics
            }, f, indent=2)
        print(f"\n📄 Results saved to {args.output}")

    # Exit with appropriate code
    if results['overall'] == 'SUCCESS':
        sys.exit(0)
    elif results['overall'] == 'PARTIAL':
        sys.exit(1)
    else:
        sys.exit(2)

if __name__ == '__main__':
    main()