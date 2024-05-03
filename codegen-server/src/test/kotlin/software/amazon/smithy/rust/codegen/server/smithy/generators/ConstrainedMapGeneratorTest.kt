/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */

package software.amazon.smithy.rust.codegen.server.smithy.generators

import io.kotest.matchers.string.shouldContain
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.extension.ExtensionContext
import org.junit.jupiter.params.ParameterizedTest
import org.junit.jupiter.params.provider.Arguments
import org.junit.jupiter.params.provider.ArgumentsProvider
import org.junit.jupiter.params.provider.ArgumentsSource
import software.amazon.smithy.model.Model
import software.amazon.smithy.model.node.ObjectNode
import software.amazon.smithy.model.shapes.MapShape
import software.amazon.smithy.model.shapes.StringShape
import software.amazon.smithy.rust.codegen.core.rustlang.RustWriter
import software.amazon.smithy.rust.codegen.core.rustlang.rustBlock
import software.amazon.smithy.rust.codegen.core.rustlang.rustTemplate
import software.amazon.smithy.rust.codegen.core.smithy.RuntimeType
import software.amazon.smithy.rust.codegen.core.testutil.TestWorkspace
import software.amazon.smithy.rust.codegen.core.testutil.asSmithyModel
import software.amazon.smithy.rust.codegen.core.testutil.compileAndTest
import software.amazon.smithy.rust.codegen.core.testutil.unitTest
import software.amazon.smithy.rust.codegen.core.util.lookup
import software.amazon.smithy.rust.codegen.core.util.toSnakeCase
import software.amazon.smithy.rust.codegen.server.smithy.ServerCodegenContext
import software.amazon.smithy.rust.codegen.server.smithy.ServerRustModule
import software.amazon.smithy.rust.codegen.server.smithy.createTestInlineModuleCreator
import software.amazon.smithy.rust.codegen.server.smithy.customizations.SmithyValidationExceptionConversionGenerator
import software.amazon.smithy.rust.codegen.server.smithy.testutil.serverTestCodegenContext
import software.amazon.smithy.rust.codegen.server.smithy.transformers.ShapesReachableFromOperationInputTagger
import java.util.stream.Stream

class ConstrainedMapGeneratorTest {
    data class TestCase(val model: Model, val validMap: ObjectNode, val invalidMap: ObjectNode)

    class ConstrainedMapGeneratorTestProvider : ArgumentsProvider {
        private val testCases =
            listOf(
                // Min and max.
                Triple("@length(min: 11, max: 12)", 11, 13),
                // Min equal to max.
                Triple("@length(min: 11, max: 11)", 11, 12),
                // Only min.
                Triple("@length(min: 11)", 15, 10),
                // Only max.
                Triple("@length(max: 11)", 11, 12),
            ).map {
                val validStringMap = List(it.second) { index -> index.toString() to "value" }.toMap()
                val inValidStringMap = List(it.third) { index -> index.toString() to "value" }.toMap()
                Triple(it.first, ObjectNode.fromStringMap(validStringMap), ObjectNode.fromStringMap(inValidStringMap))
            }.map { (trait, validMap, invalidMap) ->
                TestCase(
                    """
                    namespace test

                    $trait
                    map ConstrainedMap {
                        key: String,
                        value: String
                    }
                    """.asSmithyModel().let(ShapesReachableFromOperationInputTagger::transform),
                    validMap,
                    invalidMap,
                )
            }

        override fun provideArguments(context: ExtensionContext?): Stream<out Arguments> =
            testCases.map { Arguments.of(it) }.stream()
    }

    @ParameterizedTest
    @ArgumentsSource(ConstrainedMapGeneratorTestProvider::class)
    fun `it should generate constrained map types`(testCase: TestCase) {
        val constrainedMapShape = testCase.model.lookup<MapShape>("test#ConstrainedMap")

        val codegenContext = serverTestCodegenContext(testCase.model)
        val symbolProvider = codegenContext.symbolProvider

        val project = TestWorkspace.testProject(symbolProvider)

        project.withModule(ServerRustModule.Model) {
            TestUtility.generateIsDisplay().invoke(this)
            TestUtility.generateIsError().invoke(this)

            render(codegenContext, this, constrainedMapShape)

            val instantiator = ServerInstantiator(codegenContext)
            rustBlock("##[cfg(test)] fn build_valid_map() -> std::collections::HashMap<String, String>") {
                instantiator.render(this, constrainedMapShape, testCase.validMap)
            }
            rustBlock("##[cfg(test)] fn build_invalid_map() -> std::collections::HashMap<String, String>") {
                instantiator.render(this, constrainedMapShape, testCase.invalidMap)
            }

            unitTest(
                name = "try_from_success",
                test = """
                    let map = build_valid_map();
                    let _constrained: ConstrainedMap = map.try_into().unwrap();
                """,
            )
            unitTest(
                name = "try_from_fail",
                test = """
                    let map = build_invalid_map();
                    let constrained_res: Result<ConstrainedMap, _> = map.try_into();
                    let error = constrained_res.unwrap_err();
    
                    is_display(&error);
                    is_error(&error);
                """,
            )
            unitTest(
                name = "inner",
                test = """
                    let map = build_valid_map();
                    let constrained = ConstrainedMap::try_from(map.clone()).unwrap();

                    assert_eq!(constrained.inner(), &map);
                """,
            )
            unitTest(
                name = "into_inner",
                test = """
                    let map = build_valid_map();
                    let constrained = ConstrainedMap::try_from(map.clone()).unwrap();

                    assert_eq!(constrained.into_inner(), map);
                """,
            )
        }

        project.compileAndTest()
    }

    @Test
    fun `type should not be constructable without using a constructor`() {
        val model =
            """
            namespace test

            @length(min: 1, max: 69)
            map ConstrainedMap {
                key: String,
                value: String
            }
            """.asSmithyModel().let(ShapesReachableFromOperationInputTagger::transform)
        val constrainedMapShape = model.lookup<MapShape>("test#ConstrainedMap")

        val writer = RustWriter.forModule(ServerRustModule.Model.name)

        val codegenContext = serverTestCodegenContext(model)
        render(codegenContext, writer, constrainedMapShape)

        // Check that the wrapped type is `pub(crate)`.
        writer.toString() shouldContain "pub struct ConstrainedMap(pub(crate) ::std::collections::HashMap<::std::string::String, ::std::string::String>);"
    }

    @Test
    fun `error trait implemented for ConstraintViolation should work for constrained key and value`() {
        val model =
            """
            ${'$'}version: "2"
            
            namespace test
            
            use aws.protocols#restJson1
            use smithy.framework#ValidationException
            
            // The `ConstraintViolation` code generated for a constrained map that is not reachable from an
            // operation does not have the `Key`, or `Value` variants. Hence, we need to define a service
            // and an operation that uses the constrained map.
            @restJson1
            service MyService {
                version: "2023-04-01",
                operations: [
                    MyOperation,
                ]
            }
            
            @http(method: "POST", uri: "/echo")
            operation MyOperation {
                input:= {
                    member1: ConstrainedMapWithConstrainedKey,
                    member2: ConstrainedMapWithConstrainedKeyAndValue,
                    member3: ConstrainedMapWithConstrainedValue,
                },
                output:= {},
                errors : [ValidationException]
            }
            
            @length(min: 2, max: 69)
            map ConstrainedMapWithConstrainedKey {
                key: ConstrainedKey,
                value: String
            }

            @length(min: 2, max: 69)
            map ConstrainedMapWithConstrainedValue {
                key: String,
                value: ConstrainedValue
            }

            @length(min: 2, max: 69)
            map ConstrainedMapWithConstrainedKeyAndValue {
                key: ConstrainedKey,
                value: ConstrainedValue,
            }

            @pattern("#\\d+")
            string ConstrainedKey

            @pattern("A-Z")
            string ConstrainedValue
            """.asSmithyModel().let(ShapesReachableFromOperationInputTagger::transform)
        val constrainedKeyShape = model.lookup<StringShape>("test#ConstrainedKey")
        val constrainedValueShape = model.lookup<StringShape>("test#ConstrainedValue")

        val codegenContext = serverTestCodegenContext(model)
        val symbolProvider = codegenContext.symbolProvider
        val project = TestWorkspace.testProject(symbolProvider)

        project.withModule(ServerRustModule.Model) {
            TestUtility.generateIsDisplay().invoke(this)
            TestUtility.generateIsError().invoke(this)
            TestUtility.renderConstrainedString(codegenContext, this, constrainedKeyShape)
            TestUtility.renderConstrainedString(codegenContext, this, constrainedValueShape)

            val mapsToVerify =
                listOf(
                    model.lookup<MapShape>("test#ConstrainedMapWithConstrainedKey"),
                    model.lookup<MapShape>("test#ConstrainedMapWithConstrainedKeyAndValue"),
                    model.lookup<MapShape>("test#ConstrainedMapWithConstrainedValue"),
                )

            rustTemplate(
                """
                fn build_invalid_constrained_map_with_constrained_key() -> #{HashMap}<ConstrainedKey, String> {
                    let mut m = ::std::collections::HashMap::new();
                    m.insert(ConstrainedKey("1".to_string()), "Y".to_string());
                    m    
                }
                fn build_invalid_constrained_map_with_constrained_key_and_value() -> std::collections::HashMap<ConstrainedKey, ConstrainedValue> {
                    let mut m = ::std::collections::HashMap::new();
                    m.insert(ConstrainedKey("1".to_string()), ConstrainedValue("Y".to_string()));
                    m
                }
                fn build_invalid_constrained_map_with_constrained_value() -> std::collections::HashMap<String, ConstrainedValue> {
                    let mut m = ::std::collections::HashMap::new();
                    m.insert("1".to_string(), ConstrainedValue("Y".to_string()));
                    m
                }
                
                // Define `ValidationExceptionField` since it is required by the `ConstraintViolation` code for constrained maps, 
                // and the complete SDK generation process, which would generate it, is not invoked as part of the test.
                pub struct ValidationExceptionField {
                    pub message: String,
                    pub path: String
                }
                """,
                "HashMap" to RuntimeType.HashMap,
            )

            for (mapToVerify in mapsToVerify) {
                val rustShapeName = mapToVerify.toShapeId().name
                val rustShapeSnakeCaseName = rustShapeName.toSnakeCase()

                render(codegenContext, this, mapToVerify)

                unitTest(
                    name = "try_from_fail_$rustShapeSnakeCaseName",
                    test = """
                        let map = build_invalid_$rustShapeSnakeCaseName();
                        let constrained_res: Result<$rustShapeName, $rustShapeSnakeCaseName::ConstraintViolation> = map.try_into();
                        let error = constrained_res.unwrap_err();
                        is_error(&error);
                        is_display(&error);
                        assert_eq!("Value with length 1 provided for 'test#$rustShapeName' failed to satisfy constraint: Member must have length between 2 and 69, inclusive", error.to_string());
                    """,
                )
            }

            unitTest(
                name = "try_constrained_key",
                test =
                    """
                    let error = constrained_map_with_constrained_key::ConstraintViolation::Key(constrained_key::ConstraintViolation::Pattern("some error".to_string()));
                    assert_eq!(error.to_string(), "Value provided for `test#ConstrainedKey` failed to satisfy the constraint: Member must match the regular expression pattern: #\\d+");
                    """,
            )
            unitTest(
                name = "try_constrained_value",
                test =
                    """
                    let error = constrained_map_with_constrained_value::ConstraintViolation::Value("some_key".to_string(), constrained_value::ConstraintViolation::Pattern("some error".to_string()));
                    assert_eq!(error.to_string(), "Value provided for `test#ConstrainedValue` failed to satisfy the constraint: Member must match the regular expression pattern: A-Z");
                    """,
            )
            unitTest(
                name = "try_constrained_key_and_value",
                test =
                    """
                    let error = constrained_map_with_constrained_key_and_value::ConstraintViolation::Key(constrained_key::ConstraintViolation::Pattern("some error".to_string()));
                    assert_eq!(error.to_string(), "Value provided for `test#ConstrainedKey` failed to satisfy the constraint: Member must match the regular expression pattern: #\\d+");
                    let error = constrained_map_with_constrained_key_and_value::ConstraintViolation::Value(ConstrainedKey("1".to_string()), constrained_value::ConstraintViolation::Pattern("some error".to_string()));
                    assert_eq!(error.to_string(), "Value provided for `test#ConstrainedValue` failed to satisfy the constraint: Member must match the regular expression pattern: A-Z");
                    """,
            )
        }

        project.compileAndTest()
    }

    private fun render(
        codegenContext: ServerCodegenContext,
        writer: RustWriter,
        constrainedMapShape: MapShape,
    ) {
        ConstrainedMapGenerator(codegenContext, writer, constrainedMapShape).render()
        MapConstraintViolationGenerator(
            codegenContext,
            writer.createTestInlineModuleCreator(),
            constrainedMapShape,
            SmithyValidationExceptionConversionGenerator(codegenContext),
        ).render()
    }
}
