/**
 * Commerce (§6.3).
 *
 * Products, orders as a pipeline with one-click transitions, payments
 * with honest configured state, marketplace with compatibility
 * verdicts in plain language. Revenue is a chart built ONLY from
 * paid orders returned by the API (all pages, capped at 200 orders)
 * — never invented, never estimated; when there are no paid orders
 * the chart says so instead of drawing zeros that look like data.
 * Prices are integer minor units everywhere.
 */
import { useCallback, useEffect, useState } from "react";
import { api } from "../lib/api";
import type { Coupon, WooImportReport, MarketplaceExtension, Order, OrderList, PaymentProviderInfo, Product, ProductList, SelectedPayment, ShippingZone, TaxRate, TaxSettings, WebhookLogList, WooAttribute, WooCustomer, WooProduct, WooVariation, ZoneWithMethods } from "../lib/types";
import { Alert, Badge, Button, Card, EmptyState, Input, Select, Skeleton, Table, Tabs } from "../components/elements";

export function fmtMoney(minor: number, currency: string): string {
  try {
    return new Intl.NumberFormat(undefined, { style: "currency", currency }).format(minor / 100);
  } catch {
    return `${(minor / 100).toFixed(2)} ${currency}`;
  }
}

type Tab = "products" | "orders" | "coupons" | "taxShipping" | "customers" | "import" | "payments" | "marketplace";

const NEXT_STATUS: Record<Order["status"], Array<{ to: "paid" | "fulfilled" | "cancelled"; label: string }>> = {
  pending: [
    { to: "paid", label: "Mark paid" },
    { to: "cancelled", label: "Cancel" },
  ],
  paid: [
    { to: "fulfilled", label: "Mark fulfilled" },
    { to: "cancelled", label: "Cancel (restores stock)" },
  ],
  fulfilled: [],
  cancelled: [],
};

const PIPELINE: Array<Order["status"]> = ["pending", "paid", "fulfilled", "cancelled"];

function statusTone(s: string): "teal" | "amber" | "red" | "blue" | "grey" {
  if (s === "paid" || s === "fulfilled" || s === "published") return "teal";
  if (s === "pending" || s === "draft") return "amber";
  if (s === "cancelled") return "red";
  return "grey";
}

function verdictTone(v: string): "teal" | "amber" | "red" | "grey" {
  if (v === "compatible") return "teal";
  if (v === "partial") return "amber";
  if (v === "blocked") return "red";
  return "grey";
}

export function CommercePage(): React.ReactElement {
  const [tab, setTab] = useState<Tab>("products");
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const [products, setProducts] = useState<ProductList | null>(null);
  const [productPage, setProductPage] = useState(1);
  const [name, setName] = useState("");
  const [price, setPrice] = useState("");
  const [currency, setCurrency] = useState("NGN");
  const [stock, setStock] = useState("10");
  const [status, setStatus] = useState<"draft" | "published">("draft");
  const [productType, setProductType] = useState<"simple" | "variable">("simple");
  const [sku, setSku] = useState("");
  const [savingProduct, setSavingProduct] = useState(false);

  // P8: the variable product being edited (attributes + variations).
  const [wooProduct, setWooProduct] = useState<WooProduct | null>(null);
  const [attrName, setAttrName] = useState("");
  const [attrOptions, setAttrOptions] = useState("");
  const [attrVariation, setAttrVariation] = useState(true);
  const [varSku, setVarSku] = useState("");
  const [varPrice, setVarPrice] = useState("");
  const [varSalePrice, setVarSalePrice] = useState("");
  const [varStock, setVarStock] = useState("0");
  const [varStatus, setVarStatus] = useState<"draft" | "published">("published");
  const [varChoices, setVarChoices] = useState<Record<string, string>>({});
  const [savingVariation, setSavingVariation] = useState(false);

  // P8: coupons, tax + shipping, customers, import.
  const [coupons, setCoupons] = useState<Coupon[] | null>(null);
  const [couponCode, setCouponCode] = useState("");
  const [couponType, setCouponType] = useState<"fixed" | "percent">("fixed");
  const [couponAmount, setCouponAmount] = useState("");
  const [couponPercent, setCouponPercent] = useState("");
  const [couponMinOrder, setCouponMinOrder] = useState("");
  const [couponMaxUses, setCouponMaxUses] = useState("");
  const [couponExpires, setCouponExpires] = useState("");
  const [savingCoupon, setSavingCoupon] = useState(false);
  const [taxRates, setTaxRates] = useState<TaxRate[] | null>(null);
  const [taxSettings, setTaxSettings] = useState<TaxSettings | null>(null);
  const [taxName, setTaxName] = useState("");
  const [taxCountry, setTaxCountry] = useState("");
  const [taxState, setTaxState] = useState("");
  const [taxRateBp, setTaxRateBp] = useState("");
  const [savingTax, setSavingTax] = useState(false);
  const [zones, setZones] = useState<ZoneWithMethods[] | null>(null);
  const [zoneName, setZoneName] = useState("");
  const [zoneCountries, setZoneCountries] = useState("");
  const [methodZoneId, setMethodZoneId] = useState("");
  const [methodName, setMethodName] = useState("");
  const [methodType, setMethodType] = useState<"flat_rate" | "free_shipping">("flat_rate");
  const [methodCost, setMethodCost] = useState("");
  const [methodMinOrder, setMethodMinOrder] = useState("");
  const [savingShipping, setSavingShipping] = useState(false);
  const [customers, setCustomers] = useState<{ customers: WooCustomer[]; total: number; page: number; perPage: number } | null>(null);
  const [customerPage, setCustomerPage] = useState(1);
  const [importKind, setImportKind] = useState<"products" | "orders" | "customers">("products");
  const [importCsv, setImportCsv] = useState("");
  const [importReport, setImportReport] = useState<WooImportReport | null>(null);
  const [importing, setImporting] = useState(false);

  const [orders, setOrders] = useState<OrderList | null>(null);
  const [orderPage, setOrderPage] = useState(1);
  const [selectedOrder, setSelectedOrder] = useState<Order | null>(null);
  const [pipelineFilter, setPipelineFilter] = useState<Order["status"] | "all">("all");
  const [revenue, setRevenue] = useState<Array<{ day: string; totalMinor: number; count: number }> | null>(null);
  const [revenueCurrency, setRevenueCurrency] = useState("NGN");
  const [revenueNote, setRevenueNote] = useState("");

  const [providers, setProviders] = useState<PaymentProviderInfo[]>([]);
  const [selected, setSelected] = useState<SelectedPayment | null>(null);
  const [providerPick, setProviderPick] = useState("");
  const [modePick, setModePick] = useState<"test" | "live">("test");
  const [webhooks, setWebhooks] = useState<WebhookLogList | null>(null);
  const [webhookPage, setWebhookPage] = useState(1);

  const [extensions, setExtensions] = useState<MarketplaceExtension[]>([]);
  const [zipFile, setZipFile] = useState<File | null>(null);
  const [installing, setInstalling] = useState(false);

  const fail = (err: unknown): void => {
    setError(err instanceof Error ? err.message : "Something went wrong.");
  };

  const loadProducts = useCallback(async (page: number) => {
    try {
      setProducts(await api.listProducts(page));
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load products.");
    }
  }, []);

  const loadOrders = useCallback(async (page: number) => {
    try {
      setOrders(await api.listOrders(page));
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load orders.");
    }
  }, []);

  /** Revenue chart data: paid orders only, fetched across pages (max 200 orders). */
  const loadRevenue = useCallback(async () => {
    try {
      const all: Order[] = [];
      let page = 1;
      let totalPages = 1;
      while (page <= totalPages && page <= 10 && all.length < 200) {
        const list = await api.listOrders(page);
        totalPages = list.totalPages;
        all.push(...list.orders);
        page += 1;
      }
      const paidAll = all.filter((o) => o.status === "paid" || o.status === "fulfilled");
      // Never sum across currencies: chart the currency of the first paid order only.
      const chartCurrency = paidAll[0]?.currency ?? "NGN";
      const paid = paidAll.filter((o) => o.currency === chartCurrency);
      const byDay = new Map<string, { totalMinor: number; count: number }>();
      for (const o of paid) {
        const day = o.createdAt.slice(0, 10);
        const cur = byDay.get(day) ?? { totalMinor: 0, count: 0 };
        cur.totalMinor += o.totalMinor;
        cur.count += 1;
        byDay.set(day, cur);
      }
      const days = [...byDay.entries()]
        .sort((a, b) => (a[0] < b[0] ? -1 : 1))
        .slice(-14)
        .map(([day, v]) => ({ day, totalMinor: v.totalMinor, count: v.count }));
      setRevenue(days);
      setRevenueCurrency(chartCurrency);
      setRevenueNote(
        paid.length === 0
          ? `No paid orders yet — nothing to chart. (${all.length} order(s) checked in total.)`
          : `From ${paid.length} paid order(s) in ${chartCurrency} across ${all.length} order(s) checked${paidAll.length > paid.length ? ` (${paidAll.length - paid.length} paid order(s) in another currency are not mixed into this chart)` : ""}. Unpaid, pending and cancelled orders are never counted as revenue.`,
      );
    } catch {
      setRevenue(null);
      setRevenueNote("Revenue could not be loaded.");
    }
  }, []);

  const loadPayments = useCallback(async () => {
    try {
      const list = await api.listPaymentProviders();
      setProviders(list);
      const sel = await api.getSelectedPayment();
      setSelected(sel);
      setProviderPick(sel.provider);
      setModePick(sel.mode);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load payment providers.");
    }
  }, []);

  const loadWebhooks = useCallback(async (page: number) => {
    try {
      setWebhooks(await api.listWebhookLog(page));
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load the webhook log.");
    }
  }, []);

  const loadMarketplace = useCallback(async () => {
    try {
      setExtensions(await api.listMarketplace());
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load marketplace extensions.");
    }
  }, []);

  /** P8 loaders. */
  const loadCoupons = useCallback(async () => {
    try {
      setCoupons(await api.listCoupons());
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load coupons.");
    }
  }, []);

  const loadTaxShipping = useCallback(async () => {
    try {
      setTaxRates(await api.listTaxRates());
      setTaxSettings(await api.getTaxSettings());
      setZones(await api.listShippingZones());
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load tax and shipping.");
    }
  }, []);

  const loadCustomers = useCallback(async (page: number) => {
    try {
      setCustomers(await api.listCustomers(page));
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load customers.");
    }
  }, []);

  const loadWooProduct = useCallback(async (id: number) => {
    try {
      const detail = await api.getWooProduct(id);
      setWooProduct(detail);
      const choices: Record<string, string> = {};
      for (const a of detail.attributes.filter((x) => x.isVariation)) {
        choices[a.name] = a.options[0] ?? "";
      }
      setVarChoices(choices);
    } catch (err) {
      fail(err);
    }
  }, []);

  useEffect(() => { void loadProducts(productPage); }, [productPage, loadProducts]);
  useEffect(() => { void loadOrders(orderPage); }, [orderPage, loadOrders]);
  useEffect(() => { void loadRevenue(); }, [loadRevenue]);
  useEffect(() => { void loadPayments(); }, [loadPayments]);
  useEffect(() => { void loadWebhooks(webhookPage); }, [webhookPage, loadWebhooks]);
  useEffect(() => { void loadMarketplace(); }, [loadMarketplace]);
  useEffect(() => { void loadCoupons(); }, [loadCoupons]);
  useEffect(() => { void loadTaxShipping(); }, [loadTaxShipping]);
  useEffect(() => { void loadCustomers(customerPage); }, [customerPage, loadCustomers]);

  const createProduct = async (): Promise<void> => {
    setError(null);
    setNotice(null);
    const major = Number(price);
    if (!Number.isFinite(major) || major < 0) {
      setError("Enter a valid price (0 or more).");
      return;
    }
    const priceMinor = Math.round(major * 100);
    if (Math.abs(major * 100 - priceMinor) > 1e-9) {
      setError("The price must be a whole number of minor units (kobo/cents) — no fractions of a kobo.");
      return;
    }
    const stockNum = Number(stock);
    if (!Number.isInteger(stockNum) || stockNum < -1) {
      setError("Stock must be a whole number (-1 = unlimited).");
      return;
    }
    setSavingProduct(true);
    try {
      // P8: variable products and SKUs go through the Woo endpoint so
      // attributes can be added afterwards; simple products behave
      // exactly as before.
      const created = await api.createWooProduct({
        name: name.trim(),
        priceMinor,
        currency: currency.trim().toUpperCase() || "NGN",
        stock: stockNum,
        status,
        type: productType,
        sku: sku.trim() || undefined,
      });
      setNotice(`Product created: ${created.name} (${fmtMoney(created.priceMinor, created.currency)}).`);
      setName("");
      setPrice("");
      setStock("10");
      setStatus("draft");
      setProductType("simple");
      setSku("");
      await loadProducts(productPage);
    } catch (err) {
      fail(err);
    } finally {
      setSavingProduct(false);
    }
  };

  const deleteProduct = async (id: number, productName: string): Promise<void> => {
    if (!window.confirm(`Delete product "${productName}"? Products that appear in an order cannot be deleted.`)) return;
    setError(null);
    try {
      // P8: the Woo delete also removes the product's attributes,
      // variations and imported terms.
      await api.deleteWooProduct(id);
      if (wooProduct?.id === id) setWooProduct(null);
      setNotice(`Product "${productName}" deleted.`);
      await loadProducts(productPage);
    } catch (err) {
      fail(err);
    }
  };

  /** P8: attribute + variation editors for the selected variable product. */
  const createAttribute = async (): Promise<void> => {
    if (wooProduct === null) return;
    const options = attrOptions.split(",").map((s) => s.trim()).filter((s) => s !== "");
    if (attrName.trim() === "" || options.length === 0) {
      setError("An attribute needs a name and at least one option (comma-separated).");
      return;
    }
    setError(null);
    try {
      await api.createAttribute(wooProduct.id, { name: attrName.trim(), options, isVariation: attrVariation, visible: true });
      setAttrName("");
      setAttrOptions("");
      await loadWooProduct(wooProduct.id);
    } catch (err) {
      fail(err);
    }
  };

  const deleteAttribute = async (attr: WooAttribute): Promise<void> => {
    if (wooProduct === null) return;
    if (!window.confirm(`Delete attribute "${attr.name}"? Variations using it must be deleted first.`)) return;
    setError(null);
    try {
      await api.deleteAttribute(wooProduct.id, attr.id);
      await loadWooProduct(wooProduct.id);
    } catch (err) {
      fail(err);
    }
  };

  const createVariation = async (): Promise<void> => {
    if (wooProduct === null) return;
    const major = Number(varPrice);
    const saleMajor = varSalePrice.trim() === "" ? 0 : Number(varSalePrice);
    const stockNum = Number(varStock);
    if (!Number.isFinite(major) || major < 0 || !Number.isFinite(saleMajor) || saleMajor < 0 || !Number.isInteger(stockNum) || stockNum < 0) {
      setError("Enter a valid price (0 or more), sale price (0 or blank) and stock (whole number, 0 or more).");
      return;
    }
    setError(null);
    setNotice(null);
    setSavingVariation(true);
    try {
      const created = await api.createVariation(wooProduct.id, {
        sku: varSku.trim() || undefined,
        priceMinor: Math.round(major * 100),
        salePriceMinor: Math.round(saleMajor * 100),
        stock: stockNum,
        status: varStatus,
        attributes: varChoices,
      });
      setNotice(`Variation created: ${created.label} (${fmtMoney(created.effectivePriceMinor, wooProduct.currency)}).`);
      setVarSku("");
      setVarPrice("");
      setVarSalePrice("");
      setVarStock("0");
      await loadWooProduct(wooProduct.id);
      await loadProducts(productPage);
    } catch (err) {
      fail(err);
    } finally {
      setSavingVariation(false);
    }
  };

  const deleteVariation = async (v: WooVariation): Promise<void> => {
    if (wooProduct === null) return;
    if (!window.confirm(`Delete variation "${v.label}"? Variations in an order cannot be deleted.`)) return;
    setError(null);
    try {
      await api.deleteProductVariation(wooProduct.id, v.id);
      await loadWooProduct(wooProduct.id);
      await loadProducts(productPage);
    } catch (err) {
      fail(err);
    }
  };

  /** P8: coupons. */
  const createCoupon = async (): Promise<void> => {
    const amountMajor = couponType === "fixed" ? Number(couponAmount) : 0;
    const percent = couponType === "percent" ? Number(couponPercent) : 0;
    const minMajor = couponMinOrder.trim() === "" ? 0 : Number(couponMinOrder);
    const maxUses = couponMaxUses.trim() === "" ? 0 : Number(couponMaxUses);
    if (couponCode.trim() === "" || (couponType === "fixed" && (!Number.isFinite(amountMajor) || amountMajor <= 0)) || (couponType === "percent" && (!Number.isFinite(percent) || percent <= 0 || percent > 100)) || !Number.isFinite(minMajor) || minMajor < 0 || !Number.isInteger(maxUses) || maxUses < 0) {
      setError("Enter a code, and a fixed amount above 0 or a percent between 0 and 100. Minimum order and max uses are optional (blank = none).");
      return;
    }
    setError(null);
    setNotice(null);
    setSavingCoupon(true);
    try {
      const created = await api.createCoupon({
        code: couponCode.trim(),
        type: couponType,
        amountMinor: Math.round(amountMajor * 100),
        percentBp: Math.round(percent * 100),
        minOrderMinor: Math.round(minMajor * 100),
        maxUses,
        expiresAt: couponExpires.trim() === "" ? null : couponExpires.trim(),
        active: true,
      });
      setNotice(`Coupon created: ${created.code}.`);
      setCouponCode("");
      setCouponAmount("");
      setCouponPercent("");
      setCouponMinOrder("");
      setCouponMaxUses("");
      setCouponExpires("");
      await loadCoupons();
    } catch (err) {
      fail(err);
    } finally {
      setSavingCoupon(false);
    }
  };

  const toggleCoupon = async (c: Coupon): Promise<void> => {
    setError(null);
    try {
      await api.updateCoupon(c.id, { active: !c.active });
      await loadCoupons();
    } catch (err) {
      fail(err);
    }
  };

  const deleteCoupon = async (c: Coupon): Promise<void> => {
    if (!window.confirm(`Delete coupon "${c.code}"? Orders that already used it keep their snapshot.`)) return;
    setError(null);
    try {
      await api.deleteCoupon(c.id);
      await loadCoupons();
    } catch (err) {
      fail(err);
    }
  };

  /** P8: tax settings + rates. */
  const saveTaxSettings = async (): Promise<void> => {
    if (taxSettings === null) return;
    setError(null);
    setNotice(null);
    try {
      const saved = await api.setTaxSettings(taxSettings);
      setTaxSettings(saved);
      setNotice(saved.taxEnabled ? "Tax is on. Existing P3 checkout totals now include tax where a rate matches." : "Tax is off.");
    } catch (err) {
      fail(err);
    }
  };

  const createTaxRate = async (): Promise<void> => {
    const bp = Number(taxRateBp);
    if (taxName.trim() === "" || !Number.isFinite(bp) || bp < 0 || bp > 10000) {
      setError("Enter a name and a rate in basis points (750 = 7.5%).");
      return;
    }
    setError(null);
    setSavingTax(true);
    try {
      await api.createTaxRate({ name: taxName.trim(), country: taxCountry.trim(), state: taxState.trim(), rateBp: Math.round(bp) });
      setTaxName("");
      setTaxCountry("");
      setTaxState("");
      setTaxRateBp("");
      await loadTaxShipping();
    } catch (err) {
      fail(err);
    } finally {
      setSavingTax(false);
    }
  };

  const deleteTaxRate = async (r: TaxRate): Promise<void> => {
    if (!window.confirm(`Delete tax rate "${r.name}"?`)) return;
    setError(null);
    try {
      await api.deleteTaxRate(r.id);
      await loadTaxShipping();
    } catch (err) {
      fail(err);
    }
  };

  /** P8: shipping zones + methods. */
  const createZone = async (): Promise<void> => {
    const countries = zoneCountries.split(",").map((s) => s.trim().toUpperCase()).filter((s) => s !== "");
    if (zoneName.trim() === "") {
      setError("A zone needs a name. Countries are optional — an empty list means rest of the world.");
      return;
    }
    setError(null);
    setSavingShipping(true);
    try {
      await api.createShippingZone({ name: zoneName.trim(), countries });
      setZoneName("");
      setZoneCountries("");
      await loadTaxShipping();
    } catch (err) {
      fail(err);
    } finally {
      setSavingShipping(false);
    }
  };

  const deleteZone = async (z: ShippingZone): Promise<void> => {
    if (!window.confirm(`Delete shipping zone "${z.name}" and its methods?`)) return;
    setError(null);
    try {
      await api.deleteShippingZone(z.id);
      await loadTaxShipping();
    } catch (err) {
      fail(err);
    }
  };

  const createMethod = async (): Promise<void> => {
    const zoneId = Number(methodZoneId);
    const costMajor = Number(methodCost);
    const minMajor = methodMinOrder.trim() === "" ? 0 : Number(methodMinOrder);
    if (!Number.isInteger(zoneId) || zoneId < 1 || methodName.trim() === "" || !Number.isFinite(costMajor) || costMajor < 0 || !Number.isFinite(minMajor) || minMajor < 0) {
      setError("Pick a zone, enter a method name and a cost (0 or more). Minimum order is optional.");
      return;
    }
    if (methodType === "free_shipping" && costMajor !== 0) {
      setError("A free-shipping method has no cost — set the threshold with the minimum order instead.");
      return;
    }
    setError(null);
    setSavingShipping(true);
    try {
      await api.createShippingMethod(zoneId, { name: methodName.trim(), type: methodType, costMinor: Math.round(costMajor * 100), minOrderMinor: Math.round(minMajor * 100) });
      setMethodName("");
      setMethodCost("");
      setMethodMinOrder("");
      await loadTaxShipping();
    } catch (err) {
      fail(err);
    } finally {
      setSavingShipping(false);
    }
  };

  const deleteMethod = async (zoneId: number, methodId: number, methodNameText: string): Promise<void> => {
    if (!window.confirm(`Delete shipping method "${methodNameText}"?`)) return;
    setError(null);
    try {
      await api.deleteShippingMethod(methodId, zoneId);
      await loadTaxShipping();
    } catch (err) {
      fail(err);
    }
  };

  /** P8: CSV import. */
  const runImport = async (): Promise<void> => {
    if (importCsv.trim() === "") {
      setError("Paste CSV text first.");
      return;
    }
    setError(null);
    setNotice(null);
    setImporting(true);
    try {
      const report = await api.importWoo(importKind, importCsv);
      setImportReport(report);
      setNotice(`Import finished: ${report.imported} imported, ${report.updated} updated, ${report.skipped} skipped, ${report.failed} failed (of ${report.totalRows} rows).`);
      await loadProducts(productPage);
      await loadOrders(orderPage);
      await loadCustomers(customerPage);
    } catch (err) {
      fail(err);
    } finally {
      setImporting(false);
    }
  };

  const toggleProductStatus = async (product: Product): Promise<void> => {
    setError(null);
    try {
      await api.updateProduct(product.id, { status: product.status === "published" ? "draft" : "published" });
      await loadProducts(productPage);
    } catch (err) {
      fail(err);
    }
  };

  const viewOrder = async (id: string): Promise<void> => {
    setError(null);
    try {
      setSelectedOrder(await api.getOrder(id));
    } catch (err) {
      fail(err);
    }
  };

  const transitionOrder = async (id: string, to: "paid" | "fulfilled" | "cancelled"): Promise<void> => {
    setError(null);
    setNotice(null);
    try {
      const updated = await api.transitionOrder(id, to);
      setSelectedOrder(updated);
      setNotice(`Order ${id} is now ${updated.status}.`);
      await loadOrders(orderPage);
      await loadRevenue();
    } catch (err) {
      fail(err);
    }
  };

  const savePaymentSelection = async (): Promise<void> => {
    setError(null);
    setNotice(null);
    try {
      const sel = await api.selectPayment(providerPick, modePick);
      setSelected(sel);
      setNotice(`Payment provider set to ${sel.provider || "none"} (${sel.mode}).`);
      await loadPayments();
    } catch (err) {
      fail(err);
    }
  };

  const installZip = async (): Promise<void> => {
    if (zipFile === null) {
      setError("Choose a .zip file first.");
      return;
    }
    setError(null);
    setNotice(null);
    setInstalling(true);
    try {
      const installed = await api.installMarketplaceZip(zipFile);
      setNotice(`Installed ${installed.name} v${installed.version} (inactive). Activate it to enable it.`);
      setZipFile(null);
      await loadMarketplace();
    } catch (err) {
      fail(err);
    } finally {
      setInstalling(false);
    }
  };

  const setExtensionStatus = async (slug: string, action: "activate" | "deactivate"): Promise<void> => {
    setError(null);
    try {
      await api.setMarketplaceStatus(slug, action);
      await loadMarketplace();
    } catch (err) {
      fail(err);
    }
  };

  const pipelineOrders = (orders?.orders ?? []).filter((o) => pipelineFilter === "all" || o.status === pipelineFilter);
  const maxRevenue = Math.max(1, ...(revenue ?? []).map((d) => d.totalMinor));

  return (
    <div>
      {error !== null ? <Alert tone="err" title="Commerce" onDismiss={() => setError(null)}>{error}</Alert> : null}
      {notice !== null ? <Alert tone="ok" onDismiss={() => setNotice(null)}>{notice}</Alert> : null}

      <Tabs
        ariaLabel="Commerce sections"
        value={tab}
        onChange={(id) => setTab(id as Tab)}
        tabs={[
          { id: "products", label: "Products", icon: "store" },
          { id: "orders", label: "Orders", icon: "receipt" },
          { id: "coupons", label: "Coupons", icon: "percent" },
          { id: "taxShipping", label: "Tax & shipping", icon: "deliveryTruck" },
          { id: "customers", label: "Customers", icon: "users" },
          { id: "import", label: "Import", icon: "upload" },
          { id: "payments", label: "Payments", icon: "card" },
          { id: "marketplace", label: "Marketplace", icon: "package" },
        ]}
      />

      {tab === "products" ? (
        <>
          <Card title="Products" sub="Prices are stored in integer minor units (kobo/cents) — never floats. Published products appear at /shop.">
            {products === null ? (
              <Skeleton height={160} label="Loading products" />
            ) : products.products.length === 0 ? (
              <EmptyState icon="store" title="No products yet" description="Create your first product below." />
            ) : (
              <>
                <Table
                  caption="Products"
                  columns={[
                    { key: "name", label: "Name" },
                    { key: "type", label: "Type" },
                    { key: "price", label: "Price" },
                    { key: "stock", label: "Stock" },
                    { key: "status", label: "Status" },
                    { key: "actions", label: "Actions" },
                  ]}
                  rows={products.products.map((p) => ({
                    name: <span><a href={`/shop/${p.slug}`} target="_blank" rel="noreferrer" className="row-title">{p.name}</a>{p.sku ? <span className="row-sub">SKU {p.sku}</span> : null}</span>,
                    type: (p.type ?? "simple") === "variable" ? <Badge tone="blue">variable</Badge> : "simple",
                    price: (p.type ?? "simple") === "variable" ? `From ${fmtMoney(p.priceMinor, p.currency)}` : fmtMoney(p.priceMinor, p.currency),
                    stock: (p.type ?? "simple") === "variable" ? "per variation" : (p.stock < 0 ? "Unlimited" : String(p.stock)),
                    status: <Badge tone={statusTone(p.status)}>{p.status}</Badge>,
                    actions: (
                      <span className="actions">
                        {(p.type ?? "simple") === "variable" ? <Button size="small" onClick={() => void loadWooProduct(p.id)}>Variations</Button> : null}
                        <Button size="small" onClick={() => void toggleProductStatus(p)}>{p.status === "published" ? "Unpublish" : "Publish"}</Button>
                        <Button size="small" variant="danger" onClick={() => void deleteProduct(p.id, p.name)}>Delete</Button>
                      </span>
                    ),
                  }))}
                />
                <div className="pagination">
                  <Button size="small" icon="chevronLeft" disabled={products.page <= 1} onClick={() => setProductPage(products.page - 1)}>Previous</Button>
                  <span>Page {products.page} of {Math.max(products.totalPages, 1)} ({products.total} total)</span>
                  <Button size="small" icon="chevronRight" disabled={products.page >= products.totalPages} onClick={() => setProductPage(products.page + 1)}>Next</Button>
                </div>
              </>
            )}
          </Card>
          <Card title="New product" sub="Variable products get their price from their variations — the price here is the starting (lowest) price until variations are added.">
            <div className="form-grid">
              <Input label="Name" value={name} onChange={(e) => setName(e.target.value)} maxLength={200} />
              <Input label="Price (major units, e.g. 2500.00)" value={price} onChange={(e) => setPrice(e.target.value)} inputMode="decimal" placeholder="0.00" />
              <Input label="Currency" value={currency} onChange={(e) => setCurrency(e.target.value)} maxLength={3} />
              <Input label="Stock (-1 = unlimited; ignored for variable products)" value={stock} onChange={(e) => setStock(e.target.value)} inputMode="numeric" />
              <Input label="SKU (optional, unique)" value={sku} onChange={(e) => setSku(e.target.value)} maxLength={100} />
              <Select label="Type" value={productType} onChange={(e) => setProductType(e.target.value as "simple" | "variable")}>
                <option value="simple">Simple</option>
                <option value="variable">Variable (attributes + variations)</option>
              </Select>
              <Select label="Status" value={status} onChange={(e) => setStatus(e.target.value as "draft" | "published")}>
                <option value="draft">Draft</option>
                <option value="published">Published</option>
              </Select>
            </div>
            <Button variant="primary" icon="plus" disabled={savingProduct} onClick={() => void createProduct()}>{savingProduct ? "Creating…" : "Create product"}</Button>
          </Card>
          {wooProduct !== null ? (
            <Card title={`Variable product: ${wooProduct.name}`} sub="Attributes are the option lists (e.g. Size: S, M, L). Variations are the purchasable combinations — each has its own price, sale price and stock. The product price is the lowest variation price." actions={<Button size="small" icon="x" aria-label="Close variable product" onClick={() => setWooProduct(null)}>Close</Button>}>
              <h3>Attributes</h3>
              {wooProduct.attributes.length === 0 ? (
                <EmptyState icon="list" title="No attributes yet" description="Add one below — variations cannot be created until the product has variation attributes." />
              ) : (
                <Table
                  caption="Attributes"
                  columns={[
                    { key: "name", label: "Name" },
                    { key: "options", label: "Options" },
                    { key: "variation", label: "Picks a variation" },
                    { key: "actions", label: "Actions" },
                  ]}
                  rows={wooProduct.attributes.map((a) => ({
                    name: a.name,
                    options: a.options.join(", "),
                    variation: a.isVariation ? "Yes" : "No",
                    actions: <Button size="small" variant="danger" onClick={() => void deleteAttribute(a)}>Delete</Button>,
                  }))}
                />
              )}
              <div className="form-grid">
                <Input label="Attribute name (e.g. Size)" value={attrName} onChange={(e) => setAttrName(e.target.value)} maxLength={100} />
                <Input label="Options (comma-separated, e.g. S, M, L)" value={attrOptions} onChange={(e) => setAttrOptions(e.target.value)} maxLength={500} />
                <Select label="Used to pick a variation" value={attrVariation ? "yes" : "no"} onChange={(e) => setAttrVariation(e.target.value === "yes")}>
                  <option value="yes">Yes</option>
                  <option value="no">No (shown on the product page only)</option>
                </Select>
              </div>
              <Button variant="primary" icon="plus" onClick={() => void createAttribute()}>Add attribute</Button>
              <h3>Variations</h3>
              {wooProduct.variations.length === 0 ? (
                <EmptyState icon="layers" title="No variations yet" description="Add one below — pick a value for each variation attribute." />
              ) : (
                <Table
                  caption="Variations"
                  columns={[
                    { key: "label", label: "Variation" },
                    { key: "price", label: "Price" },
                    { key: "stock", label: "Stock" },
                    { key: "status", label: "Status" },
                    { key: "actions", label: "Actions" },
                  ]}
                  rows={wooProduct.variations.map((v) => ({
                    label: <span className="row-title">{v.label}{v.sku ? <span className="row-sub">SKU {v.sku}</span> : null}</span>,
                    price: v.salePriceMinor > 0 ? `${fmtMoney(v.salePriceMinor, wooProduct.currency)} (was ${fmtMoney(v.priceMinor, wooProduct.currency)})` : fmtMoney(v.priceMinor, wooProduct.currency),
                    stock: String(v.stock),
                    status: <Badge tone={statusTone(v.status)}>{v.status}</Badge>,
                    actions: <Button size="small" variant="danger" onClick={() => void deleteVariation(v)}>Delete</Button>,
                  }))}
                />
              )}
              {wooProduct.attributes.filter((a) => a.isVariation).length === 0 ? null : (
                <>
                  <div className="form-grid">
                    <Input label="SKU (optional)" value={varSku} onChange={(e) => setVarSku(e.target.value)} maxLength={100} />
                    <Input label="Price (major units)" value={varPrice} onChange={(e) => setVarPrice(e.target.value)} inputMode="decimal" placeholder="0.00" />
                    <Input label="Sale price (optional, major units)" value={varSalePrice} onChange={(e) => setVarSalePrice(e.target.value)} inputMode="decimal" placeholder="0.00" />
                    <Input label="Stock" value={varStock} onChange={(e) => setVarStock(e.target.value)} inputMode="numeric" />
                    <Select label="Status" value={varStatus} onChange={(e) => setVarStatus(e.target.value as "draft" | "published")}>
                      <option value="published">Published</option>
                      <option value="draft">Draft</option>
                    </Select>
                    {wooProduct.attributes.filter((a) => a.isVariation).map((a) => (
                      <Select key={a.id} label={a.name} value={varChoices[a.name] ?? a.options[0] ?? ""} onChange={(e) => setVarChoices((cur) => ({ ...cur, [a.name]: e.target.value }))}>
                        {a.options.map((o) => <option key={o} value={o}>{o}</option>)}
                      </Select>
                    ))}
                  </div>
                  <Button variant="primary" icon="plus" disabled={savingVariation} onClick={() => void createVariation()}>{savingVariation ? "Creating…" : "Add variation"}</Button>
                </>
              )}
            </Card>
          ) : null}
        </>
      ) : null}

      {tab === "orders" ? (
        <>
          <Card title="Revenue — paid orders only" sub="Daily totals for the last 14 days with a paid order.">
            {revenue === null ? (
              <Skeleton height={120} label="Loading revenue" />
            ) : revenue.length === 0 ? (
              <EmptyState icon="chart" title="No revenue to chart yet" description={revenueNote} />
            ) : (
              <>
                <div className="bar-chart" role="img" aria-label={`Revenue by day in ${revenueCurrency}: ${revenue.map((d) => `${d.day} ${fmtMoney(d.totalMinor, revenueCurrency)} from ${d.count} order(s)`).join("; ")}`}>
                  {revenue.map((d) => (
                    <div key={d.day} className="bar-col">
                      <span className="bar-value">{fmtMoney(d.totalMinor, revenueCurrency)}</span>
                      <span className="bar" style={{ height: `${Math.max(4, Math.round((d.totalMinor / maxRevenue) * 120))}px` }} />
                      <span className="bar-label">{d.day.slice(5)}</span>
                    </div>
                  ))}
                </div>
                <p className="hint">{revenueNote}</p>
              </>
            )}
          </Card>
          <Card title="Orders pipeline" sub="Stock decrements exactly once when an order becomes paid, and is restored when a paid order is cancelled.">
            <div className="pipeline" role="group" aria-label="Filter orders by status">
              <Button size="small" variant={pipelineFilter === "all" ? "primary" : "secondary"} onClick={() => setPipelineFilter("all")}>All</Button>
              {PIPELINE.map((s) => (
                <Button key={s} size="small" variant={pipelineFilter === s ? "primary" : "secondary"} onClick={() => setPipelineFilter(s)}>{s}</Button>
              ))}
            </div>
            {orders === null ? (
              <Skeleton height={160} label="Loading orders" />
            ) : pipelineOrders.length === 0 ? (
              <EmptyState icon="receipt" title={pipelineFilter === "all" ? "No orders yet" : `No ${pipelineFilter} orders`} description="Orders appear here once a customer checks out." />
            ) : (
              <Table
                caption="Orders"
                columns={[
                  { key: "order", label: "Order" },
                  { key: "customer", label: "Customer" },
                  { key: "total", label: "Total" },
                  { key: "status", label: "Status" },
                  { key: "actions", label: "Actions" },
                ]}
                rows={pipelineOrders.map((o) => ({
                  order: <span className="row-title">{o.id.slice(0, 13)}…</span>,
                  customer: <span>{o.customerName}<span className="row-sub">{o.customerEmail}</span></span>,
                  total: fmtMoney(o.totalMinor, o.currency),
                  status: <Badge tone={statusTone(o.status)}>{o.status}</Badge>,
                  actions: (
                    <span className="actions">
                      <Button size="small" onClick={() => void viewOrder(o.id)}>View</Button>
                      {NEXT_STATUS[o.status].map((n) => (
                        <Button key={n.to} size="small" variant={n.to === "cancelled" ? "danger" : "secondary"} onClick={() => void transitionOrder(o.id, n.to)}>{n.label}</Button>
                      ))}
                    </span>
                  ),
                }))}
              />
            )}
            <div className="pagination">
              <Button size="small" icon="chevronLeft" disabled={(orders?.page ?? 1) <= 1} onClick={() => setOrderPage((orders?.page ?? 1) - 1)}>Previous</Button>
              <span>Page {orders?.page ?? 1} of {Math.max(orders?.totalPages ?? 1, 1)} ({orders?.total ?? 0} total)</span>
              <Button size="small" icon="chevronRight" disabled={(orders?.page ?? 1) >= (orders?.totalPages ?? 1)} onClick={() => setOrderPage((orders?.page ?? 1) + 1)}>Next</Button>
            </div>
          </Card>
          {selectedOrder !== null ? (
            <Card title={`Order ${selectedOrder.id}`} sub={`${selectedOrder.customerName} — ${selectedOrder.customerEmail}`} actions={<Button size="small" icon="x" aria-label="Close order" onClick={() => setSelectedOrder(null)}>Close</Button>}>
              <p>Status: <Badge tone={statusTone(selectedOrder.status)}>{selectedOrder.status}</Badge> · Total: {fmtMoney(selectedOrder.totalMinor, selectedOrder.currency)} · Provider: {selectedOrder.paymentProvider || "—"} · Reference: {selectedOrder.paymentReference || "—"}{selectedOrder.source ? ` · Source: ${selectedOrder.source}${selectedOrder.sourceId ? ` #${selectedOrder.sourceId}` : ""}` : ""}</p>
              {selectedOrder.couponCode || (selectedOrder.discountMinor ?? 0) > 0 || (selectedOrder.taxMinor ?? 0) > 0 || (selectedOrder.shippingMinor ?? 0) > 0 ? (
                <p>Subtotal: {fmtMoney(selectedOrder.subtotalMinor, selectedOrder.currency)}{selectedOrder.couponCode ? ` · Coupon ${selectedOrder.couponCode}: −${fmtMoney(selectedOrder.discountMinor ?? 0, selectedOrder.currency)}` : ""}{(selectedOrder.taxMinor ?? 0) > 0 ? ` · Tax: ${fmtMoney(selectedOrder.taxMinor ?? 0, selectedOrder.currency)}` : ""}{(selectedOrder.shippingMinor ?? 0) > 0 ? ` · Shipping${selectedOrder.shippingMethod ? ` (${selectedOrder.shippingMethod})` : ""}: ${fmtMoney(selectedOrder.shippingMinor ?? 0, selectedOrder.currency)}` : ""}</p>
              ) : null}
              <ul>
                {selectedOrder.items.map((item) => (
                  <li key={item.id}>{item.name}{item.variationLabel ? ` — ${item.variationLabel}` : ""} × {item.quantity} — {fmtMoney(item.priceMinor * item.quantity, selectedOrder.currency)}</li>
                ))}
              </ul>
              <div className="actions">
                {NEXT_STATUS[selectedOrder.status].map((n) => (
                  <Button key={n.to} variant={n.to === "cancelled" ? "danger" : "primary"} onClick={() => void transitionOrder(selectedOrder.id, n.to)}>{n.label}</Button>
                ))}
              </div>
            </Card>
          ) : null}
        </>
      ) : null}

      {tab === "coupons" ? (
        <>
          <Card title="Coupons" sub="Fixed-amount or percent-off, with usage limits, expiry and a minimum order. A use is counted exactly once, when the order becomes paid — never at apply time.">
            {coupons === null ? (
              <Skeleton height={120} label="Loading coupons" />
            ) : coupons.length === 0 ? (
              <EmptyState icon="percent" title="No coupons yet" description="Create one below." />
            ) : (
              <Table
                caption="Coupons"
                columns={[
                  { key: "code", label: "Code" },
                  { key: "value", label: "Value" },
                  { key: "limits", label: "Limits" },
                  { key: "uses", label: "Uses" },
                  { key: "status", label: "Status" },
                  { key: "actions", label: "Actions" },
                ]}
                rows={coupons.map((c) => ({
                  code: <span className="row-title">{c.code}</span>,
                  value: c.type === "percent" ? `${(c.percentBp / 100).toFixed(2)}% off` : `${fmtMoney(c.amountMinor, "NGN")} off`,
                  limits: `${c.minOrderMinor > 0 ? `min ${fmtMoney(c.minOrderMinor, "NGN")} · ` : ""}${c.maxUses > 0 ? `max ${c.maxUses} uses` : "unlimited"}${c.expiresAt ? ` · expires ${c.expiresAt}` : ""}`,
                  uses: String(c.usageCount),
                  status: <Badge tone={c.active ? "teal" : "grey"}>{c.active ? "active" : "inactive"}</Badge>,
                  actions: (
                    <span className="actions">
                      <Button size="small" onClick={() => void toggleCoupon(c)}>{c.active ? "Deactivate" : "Activate"}</Button>
                      <Button size="small" variant="danger" onClick={() => void deleteCoupon(c)}>Delete</Button>
                    </span>
                  ),
                }))}
              />
            )}
          </Card>
          <Card title="New coupon">
            <div className="form-grid">
              <Input label="Code (letters, digits, - and _)" value={couponCode} onChange={(e) => setCouponCode(e.target.value)} maxLength={50} />
              <Select label="Type" value={couponType} onChange={(e) => setCouponType(e.target.value as "fixed" | "percent")}>
                <option value="fixed">Fixed amount</option>
                <option value="percent">Percent</option>
              </Select>
              {couponType === "fixed" ? (
                <Input label="Amount (major units)" value={couponAmount} onChange={(e) => setCouponAmount(e.target.value)} inputMode="decimal" placeholder="0.00" />
              ) : (
                <Input label="Percent (0–100)" value={couponPercent} onChange={(e) => setCouponPercent(e.target.value)} inputMode="decimal" placeholder="10" />
              )}
              <Input label="Minimum order (major units, optional)" value={couponMinOrder} onChange={(e) => setCouponMinOrder(e.target.value)} inputMode="decimal" placeholder="0.00" />
              <Input label="Max uses (optional, blank = unlimited)" value={couponMaxUses} onChange={(e) => setCouponMaxUses(e.target.value)} inputMode="numeric" />
              <Input label="Expires (ISO date, optional)" value={couponExpires} onChange={(e) => setCouponExpires(e.target.value)} placeholder="2027-01-31" />
            </div>
            <Button variant="primary" icon="plus" disabled={savingCoupon} onClick={() => void createCoupon()}>{savingCoupon ? "Creating…" : "Create coupon"}</Button>
          </Card>
        </>
      ) : null}

      {tab === "taxShipping" ? (
        <>
          <Card title="Tax" sub="Tax is off by default and prices are exclusive by default, so existing checkout totals cannot change until you turn tax on. Rates match most-specific first: country + state, then country, then the global rate.">
            <div className="form-grid">
              <Select label="Tax enabled" value={taxSettings?.taxEnabled ? "yes" : "no"} onChange={(e) => setTaxSettings((cur) => cur === null ? cur : { ...cur, taxEnabled: e.target.value === "yes" })}>
                <option value="no">Off</option>
                <option value="yes">On</option>
              </Select>
              <Select label="Prices include tax" value={taxSettings?.pricesIncludeTax ? "yes" : "no"} onChange={(e) => setTaxSettings((cur) => cur === null ? cur : { ...cur, pricesIncludeTax: e.target.value === "yes" })}>
                <option value="no">No — tax is added at checkout</option>
                <option value="yes">Yes — tax is inside the price</option>
              </Select>
            </div>
            <Button variant="primary" disabled={taxSettings === null} onClick={() => void saveTaxSettings()}>Save tax settings</Button>
            {taxRates === null ? (
              <Skeleton height={120} label="Loading tax rates" />
            ) : taxRates.length === 0 ? (
              <EmptyState icon="receipt" title="No tax rates yet" description="Add one below." />
            ) : (
              <Table
                caption="Tax rates"
                columns={[
                  { key: "name", label: "Name" },
                  { key: "where", label: "Applies to" },
                  { key: "rate", label: "Rate" },
                  { key: "actions", label: "Actions" },
                ]}
                rows={taxRates.map((r) => ({
                  name: r.name,
                  where: `${r.country || "any country"}${r.state ? ` / ${r.state}` : ""}`,
                  rate: `${(r.rateBp / 100).toFixed(2)}%`,
                  actions: <Button size="small" variant="danger" onClick={() => void deleteTaxRate(r)}>Delete</Button>,
                }))}
              />
            )}
            <div className="form-grid">
              <Input label="Rate name" value={taxName} onChange={(e) => setTaxName(e.target.value)} maxLength={100} />
              <Input label="Country (2-letter code, blank = any)" value={taxCountry} onChange={(e) => setTaxCountry(e.target.value)} maxLength={2} />
              <Input label="State (optional)" value={taxState} onChange={(e) => setTaxState(e.target.value)} maxLength={100} />
              <Input label="Rate (basis points, 750 = 7.5%)" value={taxRateBp} onChange={(e) => setTaxRateBp(e.target.value)} inputMode="numeric" />
            </div>
            <Button variant="primary" icon="plus" disabled={savingTax} onClick={() => void createTaxRate()}>{savingTax ? "Creating…" : "Add tax rate"}</Button>
          </Card>
          <Card title="Shipping" sub="Zones match by country (an empty country list is rest of the world). Without a method the customer pays nothing — the store ships free rather than blocking the sale.">
            {zones === null ? (
              <Skeleton height={120} label="Loading shipping zones" />
            ) : zones.length === 0 ? (
              <EmptyState icon="deliveryTruck" title="No shipping zones yet" description="Add one below." />
            ) : (
              zones.map((z) => (
                <div key={z.zone.id} className="zone-block">
                  <h3>{z.zone.name} <span className="muted">({z.zone.countries.length === 0 ? "rest of the world" : z.zone.countries.join(", ")})</span></h3>
                  {z.methods.length === 0 ? (
                    <p className="muted">No methods — orders to this zone ship free.</p>
                  ) : (
                    <Table
                      caption={`Methods in ${z.zone.name}`}
                      columns={[
                        { key: "name", label: "Method" },
                        { key: "type", label: "Type" },
                        { key: "cost", label: "Cost / threshold" },
                        { key: "actions", label: "Actions" },
                      ]}
                      rows={z.methods.map((m) => ({
                        name: m.name,
                        type: m.type === "free_shipping" ? "Free shipping" : "Flat rate",
                        cost: m.type === "free_shipping" ? `free over ${fmtMoney(m.minOrderMinor, "NGN")}` : fmtMoney(m.costMinor, "NGN"),
                        actions: <Button size="small" variant="danger" onClick={() => void deleteMethod(z.zone.id, m.id, m.name)}>Delete</Button>,
                      }))}
                    />
                  )}
                  <Button size="small" variant="danger" onClick={() => void deleteZone(z.zone)}>Delete zone</Button>
                </div>
              ))
            )}
            <div className="form-grid">
              <Input label="Zone name" value={zoneName} onChange={(e) => setZoneName(e.target.value)} maxLength={100} />
              <Input label="Countries (comma-separated 2-letter codes, blank = rest of the world)" value={zoneCountries} onChange={(e) => setZoneCountries(e.target.value)} />
            </div>
            <Button variant="primary" icon="plus" disabled={savingShipping} onClick={() => void createZone()}>Add zone</Button>
            {zones !== null && zones.length > 0 ? (
              <>
                <h3>New method</h3>
                <div className="form-grid">
                  <Select label="Zone" value={methodZoneId} onChange={(e) => setMethodZoneId(e.target.value)}>
                    <option value="">Choose a zone…</option>
                    {zones.map((z) => <option key={z.zone.id} value={String(z.zone.id)}>{z.zone.name}</option>)}
                  </Select>
                  <Input label="Method name" value={methodName} onChange={(e) => setMethodName(e.target.value)} maxLength={100} />
                  <Select label="Type" value={methodType} onChange={(e) => setMethodType(e.target.value as "flat_rate" | "free_shipping")}>
                    <option value="flat_rate">Flat rate</option>
                    <option value="free_shipping">Free shipping</option>
                  </Select>
                  <Input label="Cost (major units)" value={methodCost} onChange={(e) => setMethodCost(e.target.value)} inputMode="decimal" placeholder="0.00" />
                  <Input label="Free over (major units, free shipping only)" value={methodMinOrder} onChange={(e) => setMethodMinOrder(e.target.value)} inputMode="decimal" placeholder="0.00" />
                </div>
                <Button variant="primary" icon="plus" disabled={savingShipping} onClick={() => void createMethod()}>Add method</Button>
              </>
            ) : null}
          </Card>
        </>
      ) : null}

      {tab === "customers" ? (
        <Card title="Customers" sub="Customers are created at checkout and by customer import. Passwords are never imported — imported customers must reset their password.">
          {customers === null ? (
            <Skeleton height={160} label="Loading customers" />
          ) : customers.customers.length === 0 ? (
            <EmptyState icon="users" title="No customers yet" description="Customers appear here once someone checks out or is imported." />
          ) : (
            <>
              <Table
                caption="Customers"
                columns={[
                  { key: "name", label: "Name" },
                  { key: "email", label: "Email" },
                  { key: "country", label: "Country" },
                  { key: "orders", label: "Orders" },
                  { key: "spent", label: "Total spent" },
                ]}
                rows={customers.customers.map((c) => ({
                  name: `${c.firstName} ${c.lastName}`.trim() || "—",
                  email: c.email,
                  country: c.country || "—",
                  orders: String(c.totalOrders),
                  spent: fmtMoney(c.totalSpentMinor, "NGN"),
                }))}
              />
              <div className="pagination">
                <Button size="small" icon="chevronLeft" disabled={customers.page <= 1} onClick={() => setCustomerPage(customers.page - 1)}>Previous</Button>
                <span>Page {customers.page} of {Math.max(Math.ceil(customers.total / Math.max(customers.perPage, 1)), 1)} ({customers.total} total)</span>
                <Button size="small" icon="chevronRight" disabled={customers.page >= Math.ceil(customers.total / Math.max(customers.perPage, 1))} onClick={() => setCustomerPage(customers.page + 1)}>Next</Button>
              </div>
            </>
          )}
        </Card>
      ) : null}

      {tab === "import" ? (
        <Card title="Import from WooCommerce" sub="CSV only. Products use WooCommerce's native product-export columns; orders use the de-facto WebToffee shape (line_item_1, line_item_2, …); customers use WooCommerce's native customer-export columns. Nothing is silently dropped — every row lands in the report as imported, updated, skipped or failed, with a reason.">
          <div className="form-grid">
            <Select label="What to import" value={importKind} onChange={(e) => setImportKind(e.target.value as "products" | "orders" | "customers")}>
              <option value="products">Products</option>
              <option value="orders">Orders</option>
              <option value="customers">Customers</option>
            </Select>
          </div>
          <p className="hint">Not imported, and recorded per row: remote product images (the URL is kept as a note — upload the image in Media), downloadable files, order notes, refunds as separate records (a refunded order imports as one cancelled order), payment-gateway tokens, WooCommerce tax/shipping settings (re-enter them in Tax &amp; shipping), and customer passwords (imported customers must reset their password). Imported orders keep their original payment state and never re-decrement stock.</p>
          <label className="field-label" htmlFor="woo-import-csv">CSV text</label>
          <textarea id="woo-import-csv" className="field-textarea" rows={12} value={importCsv} onChange={(e) => setImportCsv(e.target.value)} placeholder="Paste the exported CSV here…" />
          <Button variant="primary" icon="upload" disabled={importing} onClick={() => void runImport()}>{importing ? "Importing…" : "Run import"}</Button>
          {importReport !== null ? (
            <>
              <p>{importReport.imported} imported · {importReport.updated} updated · {importReport.skipped} skipped · {importReport.failed} failed (of {importReport.totalRows} rows).</p>
              <Table
                caption="Import report"
                columns={[
                  { key: "row", label: "Row" },
                  { key: "key", label: "Key" },
                  { key: "status", label: "Status" },
                  { key: "reason", label: "Reason" },
                ]}
                rows={importReport.rows.map((r) => ({
                  row: String(r.row),
                  key: r.key,
                  status: <Badge tone={r.status === "imported" || r.status === "updated" ? "teal" : r.status === "failed" ? "red" : "grey"}>{r.status}</Badge>,
                  reason: r.reason ?? "—",
                }))}
              />
            </>
          ) : null}
        </Card>
      ) : null}

      {tab === "payments" ? (
        <>
          <Card title="Payments" sub={`Selected provider: ${selected?.provider || "none"} (${selected?.mode ?? "test"}). With no provider selected, checkout refuses with a 503 — it never fakes a successful payment. Secrets are set with wrangler secret put, never in this UI.`}>
            <div className="form-grid">
              <Select label="Provider" value={providerPick} onChange={(e) => setProviderPick(e.target.value)}>
                <option value="">None (checkout disabled)</option>
                {providers.map((p) => (
                  <option key={p.id} value={p.id}>{p.name}{p.id === "mock" ? " — test mode only" : ""}</option>
                ))}
              </Select>
              <Select label="Mode" value={modePick} onChange={(e) => setModePick(e.target.value as "test" | "live")}>
                <option value="test">Test</option>
                <option value="live">Live</option>
              </Select>
            </div>
            <Button variant="primary" onClick={() => void savePaymentSelection()}>Save provider</Button>
            <Table
              caption="Payment providers"
              columns={[
                { key: "provider", label: "Provider" },
                { key: "configured", label: "Configured" },
                { key: "detail", label: "Detail" },
              ]}
              rows={providers.map((p) => ({
                provider: p.name,
                configured: p.configured ? <Badge tone="teal">Yes</Badge> : <Badge tone="amber">No</Badge>,
                detail: p.configuredDetail,
              }))}
              empty={<EmptyState icon="card" title="No payment providers registered" />}
            />
          </Card>
          <Card title="Webhook log" sub="Every webhook attempt, accepted or rejected. Secrets are never logged.">
            {webhooks === null ? (
              <Skeleton height={120} label="Loading webhook log" />
            ) : webhooks.entries.length === 0 ? (
              <EmptyState icon="activity" title="No webhooks received yet" />
            ) : (
              <>
                <Table
                  caption="Webhook log"
                  columns={[
                    { key: "provider", label: "Provider" },
                    { key: "event", label: "Event" },
                    { key: "reference", label: "Reference" },
                    { key: "result", label: "Result" },
                    { key: "when", label: "When" },
                  ]}
                  rows={webhooks.entries.map((w) => ({ provider: w.provider, event: w.event, reference: w.reference, result: w.result, when: w.createdAt }))}
                />
                <div className="pagination">
                  <Button size="small" icon="chevronLeft" disabled={webhooks.page <= 1} onClick={() => setWebhookPage(webhooks.page - 1)}>Previous</Button>
                  <span>Page {webhooks.page} of {Math.max(webhooks.totalPages, 1)} ({webhooks.total} total)</span>
                  <Button size="small" icon="chevronRight" disabled={webhooks.page >= webhooks.totalPages} onClick={() => setWebhookPage(webhooks.page + 1)}>Next</Button>
                </div>
              </>
            )}
          </Card>
        </>
      ) : null}

      {tab === "marketplace" ? (
        <Card title="Marketplace" sub="Install extensions from a ZIP with a wordless.json manifest (≤50 MB, ≤10,000 files). Every install is analysed and auto-converted on upload. Installed code is never executed — a plugin only runs after its converted code is loaded, which is a later milestone. A plugin is never labelled as working while its report shows partial or blocked files.">
          <div className="toolbar">
            <input type="file" accept=".zip" aria-label="Extension ZIP file" onChange={(e) => setZipFile(e.target.files?.[0] ?? null)} />
            <Button variant="primary" icon="upload" disabled={installing || zipFile === null} onClick={() => void installZip()}>{installing ? "Installing…" : "Install from ZIP"}</Button>
          </div>
          {extensions.length === 0 ? (
            <EmptyState icon="package" title="No marketplace extensions installed" />
          ) : (
            <ul className="row-list">
              {extensions.map((x) => (
                <li key={x.slug} className="marketplace-row">
                  <span className="action-body">
                    <strong>{x.name}</strong> <span className="muted">v{x.version} · {x.kind} · {x.status}</span>
                    {x.report ? (
                      <>
                        <span><Badge tone={verdictTone(x.report.verdict)}>Verdict: {x.report.verdict}</Badge> {x.report.convertible} convertible · {x.report.partial} partial · {x.report.blocked} blocked · {x.report.unparsed} unparsed (of {x.report.phpFiles} PHP files)</span>
                        {x.report.conversion !== null ? <span className="muted">Auto-convert: {x.report.conversion.converted} converted · {x.report.conversion.partial} partial · {x.report.conversion.blocked} blocked; settings carried over: {x.report.conversion.settings.carriedOver.length}</span> : null}
                        {x.report.verdict === "blocked" ? <span className="field-error">Blocked — this extension cannot run. See the reasons below.</span> : null}
                        {x.report.topReasons.length > 0 ? (
                          <details>
                            <summary>Top blocking reasons</summary>
                            <ul>
                              {x.report.topReasons.map((r) => (
                                <li key={r.kind}>{r.kind} × {r.count} — {r.files.join(", ")}</li>
                              ))}
                            </ul>
                          </details>
                        ) : null}
                      </>
                    ) : (
                      <span className="muted">Never analysed yet.</span>
                    )}
                  </span>
                  <span className="actions">
                    {x.status === "active" ? (
                      <Button size="small" onClick={() => void setExtensionStatus(x.slug, "deactivate")}>Deactivate</Button>
                    ) : (
                      <Button size="small" variant="primary" onClick={() => void setExtensionStatus(x.slug, "activate")}>Activate</Button>
                    )}
                  </span>
                </li>
              ))}
            </ul>
          )}
        </Card>
      ) : null}
    </div>
  );
}
