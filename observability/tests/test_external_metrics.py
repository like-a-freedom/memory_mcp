import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from observability import check_dashboards


def metric_contract():
    return check_dashboards.ExternalMetricContract(
        metrics={
            "container_memory_rss": check_dashboards.MetricSpec(
                exporter="cadvisor",
                required_matchers={"service": "memory_mcp"},
                allowed_matchers={"service": frozenset({"memory_mcp"})},
            )
        }
    )


def descendant_panels(row):
    for panel in row.get("panels", []):
        if panel.get("type") == "row":
            yield from descendant_panels(panel)
        else:
            yield panel


class ExternalMetricContractTests(unittest.TestCase):
    def test_sanitized_yaml_contract_is_loaded(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "external_metrics.yml"
            path.write_text(
                "metrics:\n"
                "  - name: container_memory_rss\n"
                "    exporter: cadvisor\n"
                "    required_matchers:\n"
                "      service: memory_mcp\n"
                "    allowed_matchers:\n"
                "      service:\n"
                "        - memory_mcp\n"
            )

            contract = check_dashboards.load_external_metric_contract(path)

        self.assertEqual(contract, metric_contract())

    def test_empty_contract_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "external_metrics.yml"
            path.write_text("metrics: []\n")

            with self.assertRaises(ValueError):
                check_dashboards.load_external_metric_contract(path)

    def test_contract_rejects_identifying_labels(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "external_metrics.yml"
            path.write_text(
                "metrics:\n"
                "  - name: container_memory_rss\n"
                "    exporter: cadvisor\n"
                "    required_matchers:\n"
                "      service: memory_mcp\n"
                "    allowed_matchers:\n"
                "      service: [memory_mcp]\n"
                "      instance: [fixture]\n"
            )

            with self.assertRaises(ValueError):
                check_dashboards.load_external_metric_contract(path)

    def test_unverified_external_family_is_rejected(self):
        errors = check_dashboards.validate_external_expression(
            'container_memory_unverified_bytes{service="memory_mcp"}',
            metric_contract(),
        )

        self.assertEqual(len(errors), 1)

    def test_external_family_name_selector_is_rejected(self):
        errors = check_dashboards.validate_external_expression(
            '{__name__="container_memory_rss",service="memory_mcp"}', None
        )

        self.assertEqual(
            errors,
            ["external metric family selector via `__name__` is unsupported"],
        )

    def test_external_metric_zero_fallback_is_rejected(self):
        errors = check_dashboards.validate_external_expression(
            'container_memory_rss{service="memory_mcp"} or vector(0)',
            metric_contract(),
        )

        self.assertEqual(
            errors,
            ["container_memory_rss: external metric query must not add a zero fallback"],
        )

    def test_unverified_selector_label_is_rejected(self):
        errors = check_dashboards.validate_external_expression(
            'container_memory_rss{service="memory_mcp",role="api"}',
            metric_contract(),
        )

        self.assertEqual(len(errors), 1)

    def test_unverified_selector_value_is_rejected(self):
        errors = check_dashboards.validate_external_expression(
            'container_memory_rss{service="other"}', metric_contract()
        )

        self.assertEqual(len(errors), 1)

    def test_required_matcher_is_enforced(self):
        errors = check_dashboards.validate_external_expression(
            "container_memory_rss{}", metric_contract()
        )

        self.assertEqual(
            errors, ["container_memory_rss: missing required matcher `service`"]
        )

        broad_contract = check_dashboards.ExternalMetricContract(
            metrics={
                "container_memory_rss": check_dashboards.MetricSpec(
                    exporter="cadvisor",
                    required_matchers={"service": "memory_mcp"},
                    allowed_matchers={
                        "service": frozenset({"memory_mcp", "other"})
                    },
                )
            }
        )
        wrong_value_errors = check_dashboards.validate_external_expression(
            'container_memory_rss{service="other"}', broad_contract
        )

        self.assertEqual(
            wrong_value_errors,
            [
                'container_memory_rss: required matcher `service` must equal '
                '"memory_mcp"'
            ],
        )

    def test_regex_and_negative_matchers_are_rejected(self):
        regex_errors = check_dashboards.validate_external_expression(
            'container_memory_rss{service=~"memory_mcp"}', metric_contract()
        )
        negative_errors = check_dashboards.validate_external_expression(
            'container_memory_rss{service!="memory_mcp"}', metric_contract()
        )
        negative_regex_errors = check_dashboards.validate_external_expression(
            'container_memory_rss{service!~"memory_mcp"}', metric_contract()
        )

        self.assertEqual(
            regex_errors,
            ["container_memory_rss: external selector requires exact equality"],
        )
        self.assertEqual(
            negative_errors,
            ["container_memory_rss: external selector requires exact equality"],
        )
        self.assertEqual(
            negative_regex_errors,
            ["container_memory_rss: external selector requires exact equality"],
        )

    def test_verified_family_and_selector_are_accepted(self):
        errors = check_dashboards.validate_external_expression(
            'container_memory_rss{service="memory_mcp"}', metric_contract()
        )

        self.assertEqual(errors, [])

    def test_external_metric_requires_a_literal_selector(self):
        errors = check_dashboards.validate_external_expression(
            "container_memory_rss", metric_contract()
        )

        self.assertEqual(
            errors,
            ["container_memory_rss: external metric requires a literal selector"],
        )

    def test_selector_with_a_trailing_comma_is_rejected(self):
        errors = check_dashboards.validate_external_expression(
            'container_memory_rss{service="memory_mcp",}', metric_contract()
        )

        self.assertEqual(
            errors, ["container_memory_rss: external metric selector has unsupported syntax"]
        )

    def test_external_query_rejects_identifying_legend_format(self):
        document = {
            "panels": [
                {
                    "targets": [
                        {
                            "expr": 'container_memory_rss{service="memory_mcp"}',
                            "legendFormat": "{{instance}}",
                        }
                    ]
                }
            ]
        }

        self.assertEqual(
            check_dashboards.validate_dashboard_external_metrics(
                document, metric_contract()
            ),
            ["container_memory_rss: identifying legend label `instance` is forbidden"],
        )

    def test_identifying_selector_labels_are_rejected(self):
        contract = check_dashboards.ExternalMetricContract(
            metrics={
                "container_memory_rss": check_dashboards.MetricSpec(
                    exporter="cadvisor",
                    required_matchers={"service": "memory_mcp"},
                    allowed_matchers={
                        "service": frozenset({"memory_mcp"}),
                        "instance": frozenset({"synthetic-host"}),
                    },
                )
            }
        )
        errors = check_dashboards.validate_external_expression(
            'container_memory_rss{service="memory_mcp",instance="synthetic-host"}',
            contract,
        )

        self.assertEqual(
            errors,
            ["container_memory_rss: identifying selector label `instance` is forbidden"],
        )


class DashboardMetricContractTests(unittest.TestCase):
    def test_technical_dashboard_contains_http_memory_diagnostics(self):
        dashboard_path = (
            Path(__file__).resolve().parents[1]
            / "dashboards"
            / "technical.json"
        )
        document = json.loads(dashboard_path.read_text())
        row = next(
            (
                panel
                for panel in document["panels"]
                if panel.get("type") == "row"
                and panel.get("title") == "HTTP memory diagnostics"
            ),
            None,
        )

        self.assertIsNotNone(row, "technical dashboard is missing the app diagnostics row")
        self.assertTrue(row["collapsed"])
        panels = list(descendant_panels(row))
        panel_titles = {panel["title"] for panel in panels}
        self.assertIn("Fully collected body size p95", panel_titles)
        self.assertIn("Preflight refusals by reason", panel_titles)
        self.assertIn("Reserved preflight requests", panel_titles)
        self.assertIn("Reserved preflight bytes", panel_titles)
        self.assertIn("Resident tenant runtimes", panel_titles)
        self.assertIn("Resident cache-accounted bytes", panel_titles)
        self.assertIn("Background embedding task counts", panel_titles)
        self.assertIn("Background embedding retained bytes", panel_titles)
        expressions = "\n".join(
            target["expr"]
            for panel in panels
            for target in panel.get("targets", [])
        )
        self.assertIn("memory_http_preflight_body_bytes", expressions)
        self.assertIn("memory_http_preflight_refusals_total", expressions)
        self.assertIn("memory_http_preflight_reserved_requests", expressions)
        self.assertIn("memory_http_preflight_reserved_bytes", expressions)
        self.assertIn("memory_http_tenant_runtime_count", expressions)
        self.assertIn("memory_http_context_cache_accounted_bytes", expressions)
        self.assertIn("memory_http_query_cache_accounted_bytes", expressions)
        self.assertIn("memory_http_background_embedding_admitted_tasks", expressions)
        self.assertIn("memory_http_background_embedding_running_tasks", expressions)
        self.assertIn("memory_http_background_embedding_retained_bytes", expressions)
        self.assertNotIn("or vector(0)", expressions)
        self.assertFalse(
            any(
                panel.get("title") == "Host and container resources"
                for panel in document["panels"]
            ),
            "external panels require the blocked Task 15 inventory",
        )

    def test_app_body_distribution_uses_exported_metric_type(self):
        dashboard_path = (
            Path(__file__).resolve().parents[1]
            / "dashboards"
            / "technical.json"
        )
        document = json.loads(dashboard_path.read_text())
        row = next(
            panel
            for panel in document["panels"]
            if panel.get("type") == "row"
            and panel.get("title") == "HTTP memory diagnostics"
        )
        body_panel = next(
            panel
            for panel in descendant_panels(row)
            if panel.get("title") == "Fully collected body size p95"
        )
        query = body_panel["targets"][0]["expr"]

        self.assertEqual(
            query,
            'memory_http_preflight_body_bytes{quantile="0.95"}',
        )
        self.assertNotIn("_bucket", query)
        self.assertNotIn("{le=", query)

    def test_external_series_without_a_verified_contract_are_rejected(self):
        document = {
            "panels": [
                {
                    "targets": [
                        {"expr": 'node_memory_MemAvailable_bytes{job="node"}'}
                    ]
                }
            ]
        }

        errors = check_dashboards.validate_dashboard_external_metrics(document, None)

        self.assertEqual(
            errors,
            ["unverified external metric family `node_memory_MemAvailable_bytes`"],
        )

    def test_dashboard_document_walks_nested_rows(self):
        document = {
            "panels": [
                {
                    "type": "row",
                    "title": "outer",
                    "panels": [
                        {
                            "type": "row",
                            "title": "inner",
                            "panels": [
                                {
                                    "type": "timeseries",
                                    "title": "memory",
                                    "targets": [
                                        {
                                            "expr": 'container_memory_rss{service="other"}'
                                        }
                                    ],
                                }
                            ],
                        }
                    ],
                }
            ]
        }

        errors = check_dashboards.validate_dashboard_external_metrics(
            document, metric_contract()
        )

        self.assertEqual(
            errors,
            ["container_memory_rss: unverified selector value for `service`"],
        )


if __name__ == "__main__":
    unittest.main(testRunner=unittest.TextTestRunner(stream=sys.stdout))
