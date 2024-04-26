package software.amazon.smithy.rustsdk

import software.amazon.smithy.model.traits.RequestCompressionTrait
import software.amazon.smithy.rust.codegen.client.smithy.ClientCodegenContext
import software.amazon.smithy.rust.codegen.client.smithy.customize.ClientCodegenDecorator
import software.amazon.smithy.rust.codegen.client.smithy.generators.AtLeastOneServiceOperationUsesTraitIndex
import software.amazon.smithy.rust.codegen.client.smithy.generators.config.ConfigCustomization
import software.amazon.smithy.rust.codegen.client.smithy.generators.config.ServiceConfig
import software.amazon.smithy.rust.codegen.core.rustlang.rust
import software.amazon.smithy.rust.codegen.core.rustlang.rustTemplate
import software.amazon.smithy.rust.codegen.core.rustlang.writable
import software.amazon.smithy.rust.codegen.core.smithy.RuntimeType
import software.amazon.smithy.rust.codegen.core.smithy.RuntimeType.Companion.preludeScope
import software.amazon.smithy.rust.codegen.core.smithy.customize.AdHocCustomization
import software.amazon.smithy.rust.codegen.core.smithy.customize.adhocCustomization
import software.amazon.smithy.rust.codegen.core.util.thenSingletonListOf

class HttpRequestCompressionDecorator : ClientCodegenDecorator {
    override val name: String = "HttpRequestCompression"
    override val order: Byte = 0

    // Services that have at least one operation that supports request compression.
    private fun usesRequestCompression(codegenContext: ClientCodegenContext): Boolean {
        val traitIdx = AtLeastOneServiceOperationUsesTraitIndex(codegenContext.model)
        return traitIdx.usesTrait(RequestCompressionTrait.ID)
    }

    override fun configCustomizations(
        codegenContext: ClientCodegenContext,
        baseCustomizations: List<ConfigCustomization>,
    ): List<ConfigCustomization> {
        return baseCustomizations +
            usesRequestCompression(codegenContext).thenSingletonListOf {
                HttpRequestCompressionConfigCustomization(codegenContext)
            }
    }

    override fun extraSections(codegenContext: ClientCodegenContext): List<AdHocCustomization> {
        return usesRequestCompression(codegenContext).thenSingletonListOf {
            adhocCustomization<SdkConfigSection.CopySdkConfigToClientConfig> { section ->
                rust(
                    """
                    ${section.serviceConfigBuilder} = ${section.serviceConfigBuilder}
                        .disable_request_compression(${section.sdkConfig}.disable_request_compression().clone());
                    ${section.serviceConfigBuilder} = ${section.serviceConfigBuilder}
                        .request_min_compression_size_bytes(${section.sdkConfig}.request_min_compression_size_bytes().clone());
                    """,
                )
            }
        }
    }
}

class HttpRequestCompressionConfigCustomization(codegenContext: ClientCodegenContext) : ConfigCustomization() {
    private val runtimeConfig = codegenContext.runtimeConfig
    private val codegenScope =
        arrayOf(
            "Storable" to RuntimeType.smithyTypes(runtimeConfig).resolve("config_bag::Storable"),
            "StoreReplace" to RuntimeType.smithyTypes(runtimeConfig).resolve("config_bag::StoreReplace"),
            *preludeScope,
        )

    override fun section(section: ServiceConfig) =
        writable {
            when (section) {
                ServiceConfig.ConfigImpl -> {
                    rustTemplate(
                        """
                        /// Returns the TODO, if it was provided
                        pub fn disable_request_compression(&self) -> #{Option}<bool> {
                            self.config.load::<DisableRequestCompression>().map(|it| it.0)
                        }

                        /// Returns the TODO, if it was provided.
                        pub fn request_min_compression_size_bytes(&self) -> #{Option}<u32> {
                            self.config.load::<RequestMinCompressionSizeBytes>().map(|it| it.0)
                        }
                        """,
                        *codegenScope,
                    )
                }

                ServiceConfig.BuilderImpl -> {
                    rustTemplate(
                        """
                        /// Sets the TODO when making requests.
                        pub fn disable_request_compression(mut self, disable_request_compression: impl #{Into}<#{Option}<bool>>) -> Self {
                            self.set_disable_request_compression(disable_request_compression.into());
                            self
                        }

                        /// Sets the TODO when making requests.
                        pub fn request_min_compression_size_bytes(mut self, request_min_compression_size_bytes: impl #{Into}<#{Option}<u32>>) -> Self {
                            self.set_request_min_compression_size_bytes(request_min_compression_size_bytes.into());
                            self
                        }
                        """,
                        *codegenScope,
                    )

                    rustTemplate(
                        """
                        /// Sets the Todo to use when making requests.
                        pub fn set_disable_request_compression(&mut self, disable_request_compression: #{Option}<bool>) -> &mut Self {
                            self.config.store_or_unset::<DisableRequestCompression>(disable_request_compression.map(Into::into));
                            self
                        }

                        /// Sets the Todo to use when making requests.
                        pub fn set_request_min_compression_size_bytes(&mut self, request_min_compression_size_bytes: #{Option}<u32>) -> &mut Self {
                            self.config.store_or_unset::<RequestMinCompressionSizeBytes>(request_min_compression_size_bytes.map(Into::into));
                            self
                        }
                        """,
                        *codegenScope,
                    )
                }

                is ServiceConfig.BuilderFromConfigBag -> {
                    rustTemplate(
                        """
                        ${section.builder}.set_disable_request_compression(
                            ${section.configBag}.load::<DisableRequestCompression>().cloned().map(|it| it.0));
                        ${section.builder}.set_request_min_compression_size_bytes(
                            ${section.configBag}.load::<RequestMinCompressionSizeBytes>().cloned().map(|it| it.0));
                        """,
                        *codegenScope,
                    )
                }

                ServiceConfig.Extras -> {
                    rustTemplate(
                        """
                        ##[derive(Debug, Copy, Clone)]
                        struct DisableRequestCompression(bool);

                        impl From<bool> for DisableRequestCompression {
                            fn from(value: bool) -> Self {
                                DisableRequestCompression(value)
                            }
                        }

                        impl #{Storable} for DisableRequestCompression {
                            type Storer = #{StoreReplace}<Self>;
                        }

                        ##[derive(Debug, Copy, Clone)]
                        struct RequestMinCompressionSizeBytes(u32);

                        impl From<u32> for RequestMinCompressionSizeBytes {
                            fn from(value: u32) -> Self {
                                RequestMinCompressionSizeBytes(value)
                            }
                        }

                        impl #{Storable} for RequestMinCompressionSizeBytes {
                            type Storer = #{StoreReplace}<Self>;
                        }
                        """,
                        *codegenScope,
                    )
                }

                else -> emptySection
            }
        }
}